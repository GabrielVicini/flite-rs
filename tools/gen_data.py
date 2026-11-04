#!/usr/bin/env python3
"""Convert Flite's C data tables into flite-rs binary data files.

This script is a *build-time* tool, not part of the shipped library.  It reads
the upstream Flite distribution's generated C arrays (lexicon, letter-to-sound
model, CART trees, and the diphone voices) and repacks them into the
container files that flite-rs embeds:

    data/en_us.dat          language data (lexicon, LTS, CARTs, aswd FSMs)
    data/cmu_us_kal.dat     voice data (diphone index, LPC frames, residuals)
    data/cmu_us_kal16.dat   the same speaker at 16 kHz

Only *data* crosses over; none of Flite's code is translated here.  See
THIRD-PARTY-LICENSES.md for the terms those data files carry.

Usage:
    python tools/gen_data.py /path/to/flite-source [output-dir]

The container format is documented in src/data.rs.
"""

from __future__ import annotations

import os
import re
import struct
import sys

MAGIC = b"FLRSDAT\x01"

# The voices converted, and which synthesiser each asks for. All speak US
# English, so they share en_us.dat. The backend is written into the voice file
# as a `voice.backend` section, which is what the loader dispatches on: no part
# of flite-rs maps a voice name to a technique.
VOICES = [
    ("cmu_us_kal", "diphone"),
    ("cmu_us_kal16", "diphone"),
    ("cmu_us_slt", "clustergen"),
]


class Container:
    """Accumulates named byte sections and writes them as one file."""

    def __init__(self) -> None:
        self.sections: list[tuple[str, bytes]] = []

    def add(self, name: str, payload: bytes) -> None:
        assert len(name) < 256
        self.sections.append((name, payload))

    def write(self, path: str) -> None:
        with open(path, "wb") as fh:
            fh.write(MAGIC)
            for name, payload in self.sections:
                raw = name.encode("ascii")
                fh.write(struct.pack("<B", len(raw)))
                fh.write(raw)
                fh.write(struct.pack("<I", len(payload)))
                fh.write(payload)
        total = os.path.getsize(path)
        print(f"  wrote {path} ({total:,} bytes)")
        for name, payload in self.sections:
            print(f"      {name:<16} {len(payload):>10,}")


def string_table(items: list[str]) -> bytes:
    """Pack strings as a count followed by length-prefixed UTF-8 bytes."""
    out = bytearray(struct.pack("<I", len(items)))
    for s in items:
        raw = s.encode("utf-8")
        assert len(raw) < 256, s
        out.append(len(raw))
        out += raw
    return bytes(out)


# Helpers for pulling arrays and string tables out of the C sources.
def read(path: str) -> str:
    with open(path, "r", encoding="latin-1") as fh:
        return fh.read()


def strip_comments(text: str) -> str:
    return re.sub(r"/\*.*?\*/", " ", text, flags=re.S)


def array_body(text: str, name: str) -> str:
    """Return the text between `{` and the matching `}` of `<name>[...] = {`."""
    m = re.search(re.escape(name) + r"\s*\[[^\]]*\]\s*=\s*\{", text)
    if m is None:
        raise KeyError(name)
    start = m.end()
    depth = 1
    i = start
    while depth:
        c = text[i]
        if c == "{":
            depth += 1
        elif c == "}":
            depth -= 1
        i += 1
    return text[start : i - 1]


def int_array(text: str, name: str) -> list[int]:
    body = strip_comments(array_body(text, name))
    return [int(tok) for tok in re.findall(r"-?\d+", body)]


C_ESCAPES = {"n": "\n", "t": "\t", "r": "\r", "\\": "\\", '"': '"', "'": "'", "0": "\0"}


def unescape_c_string(s: str) -> bytes:
    """Decode the body of a C string literal into raw bytes."""
    out = bytearray()
    i = 0
    while i < len(s):
        c = s[i]
        if c != "\\":
            out.append(ord(c))
            i += 1
            continue
        i += 1
        c = s[i]
        if c in "01234567":
            digits = ""
            while i < len(s) and len(digits) < 3 and s[i] in "01234567":
                digits += s[i]
                i += 1
            out.append(int(digits, 8) & 0xFF)
        elif c == "x":
            i += 1
            digits = ""
            while i < len(s) and s[i] in "0123456789abcdefABCDEF":
                digits += s[i]
                i += 1
            out.append(int(digits, 16) & 0xFF)
        else:
            out.append(ord(C_ESCAPES.get(c, c)))
            i += 1
    return bytes(out)


STRING_LITERAL = re.compile(r'"((?:[^"\\]|\\.)*)"')


def string_list(text: str, name: str) -> list[str]:
    body = strip_comments(array_body(text, name))
    return [unescape_c_string(m.group(1)).decode("latin-1") for m in STRING_LITERAL.finditer(body)]


def huff_table(path: str) -> list[bytes]:
    """Read one of the lexicon's Huffman code tables (index 0 is unused)."""
    text = strip_comments(read(path))
    entries = [unescape_c_string(m.group(1)) for m in STRING_LITERAL.finditer(text)]
    return [b""] + entries  # element 0 is the reserved NULL slot


def convert_lexicon(src: str, out: Container) -> None:
    lexdir = os.path.join(src, "lang", "cmulex")
    entries_c = read(os.path.join(lexdir, "cmu_lex_entries.c"))
    phone_table = [p for p in string_list(entries_c, "cmu_lex_phone_table")]
    num_bytes = int(re.search(r"-?\d+", read(os.path.join(lexdir, "cmu_lex_num_bytes.c"))).group())

    word_huff = huff_table(os.path.join(lexdir, "cmu_lex_entries_huff_table.c"))
    phone_huff = huff_table(os.path.join(lexdir, "cmu_lex_phones_huff_table.c"))

    raw = read(os.path.join(lexdir, "cmu_lex_data_raw.c"))
    data = bytearray([0])  # cmu_lex_data.c prepends a leading 0 byte
    data += bytes(int(tok) for tok in re.findall(r"-?\d+", strip_comments(raw)))
    assert len(data) == num_bytes, (len(data), num_bytes)

    # Entries are laid out as: <phone bytes, reversed> 255 <word bytes> 0.
    # A word starts at every index whose preceding byte is 255.
    records: list[tuple[str, str, list[int]]] = []
    for index in range(1, len(data)):
        if data[index - 1] != 255:
            continue
        # Word: Huffman-coded byte string terminated by 0.
        chars = bytearray()
        p = index
        while data[p] != 0:
            chars += word_huff[data[p]]
            p += 1
        word_pos = chars.decode("latin-1")
        # Phones: scan backwards from the byte before the 255 separator.
        phones: list[int] = []
        p = index - 2
        while p >= 0 and data[p] != 0:
            phones.extend(phone_huff[data[p]])
            p -= 1
        records.append((word_pos[1:], word_pos[0], phones))

    records.sort(key=lambda r: (r[0], r[1]))
    print(f"  lexicon: {len(records)} entries, {len(phone_table)} phones")

    blob = bytearray()
    offsets = []
    for word, pos, phones in records:
        offsets.append(len(blob))
        wb = word.encode("utf-8")
        assert len(wb) < 256 and len(phones) < 256, word
        blob.append(ord(pos))
        blob.append(len(wb))
        blob += wb
        blob.append(len(phones))
        blob += bytes(phones)

    out.add("lex.phones", string_table(phone_table))
    out.add("lex.index", struct.pack("<I", len(offsets)) + b"".join(struct.pack("<I", o) for o in offsets))
    out.add("lex.data", bytes(blob))


def convert_lts(src: str, out: Container) -> None:
    lexdir = os.path.join(src, "lang", "cmulex")
    rules_c = read(os.path.join(lexdir, "cmu_lts_rules.c"))
    phone_table = string_list(rules_c, "cmu_lts_phone_table")
    letter_index = int_array(rules_c, "cmu_lts_letter_index")[:26]

    # The .h file defines every state address as a pair of little-endian bytes.
    header = read(os.path.join(lexdir, "cmu_lts_model.h"))
    states: dict[str, int] = {}
    for m in re.finditer(r"#define\s+(LTS_STATE_\w+)\s+(0x[0-9a-fA-F]+),(0x[0-9a-fA-F]+)", header):
        states[m.group(1)] = int(m.group(2), 16) | (int(m.group(3), 16) << 8)

    body = strip_comments(array_body(read(os.path.join(lexdir, "cmu_lts_model.c")), "cmu_lts_model"))
    tokens = re.findall(r"LTS_STATE_\w+|'\\?.'|-?\d+", body)

    model = bytearray()
    i = 0
    while i < len(tokens) - 6:  # the table ends with an all-zero sentinel rule
        feat = int(tokens[i])
        val_tok = tokens[i + 1]
        if val_tok.startswith("'"):
            val = ord(unescape_c_string(val_tok[1:-1]))
        else:
            val = int(val_tok) & 0xFF
        if feat == 255:  # leaf: the remaining four tokens are literal zeros
            qtrue = qfalse = 0
            i += 6
        else:
            qtrue = states[tokens[i + 2]]
            qfalse = states[tokens[i + 3]]
            i += 4
        model += struct.pack("<BBHH", feat, val, qtrue, qfalse)

    print(f"  lts: {len(model)//6} states, {len(phone_table)} phones")
    out.add("lts.index", b"".join(struct.pack("<H", a) for a in letter_index))
    out.add("lts.model", bytes(model))
    out.add("lts.phones", string_table(phone_table))


OPS = {
    "CST_CART_OP_IS": 0,
    "CST_CART_OP_IN": 1,
    "CST_CART_OP_LESS": 2,
    "CST_CART_OP_GREATER": 3,
    "CST_CART_OP_MATCHES": 4,
    "CST_CART_OP_EQUALS": 5,
    "CST_CART_OP_NONE": 255,
}


def convert_cart(src_c: str, src_h: str, symbol: str) -> bytes:
    values, consts = cart_tables(read(src_h))
    return cart_payload(read(src_c), symbol, values, consts)


def cart_tables(header_text: str) -> tuple[dict, dict]:
    """The constant values and node addresses a tree file's header defines.

    Split out so that a Clustergen voice, which holds 124 trees in one file
    behind one megabyte-sized header, pays for this once rather than 124 times.
    """
    consts: dict[str, int] = {}
    for m in re.finditer(r"#define\s+(CTNODE\w+)\s+(\d+)", header_text):
        consts[m.group(1)] = int(m.group(2))

    values: dict[str, tuple[int, object]] = {}
    for m in re.finditer(
        r'DEF_STATIC_CONST_VAL_STRING\(\s*(\w+)\s*,\s*"((?:[^"\\]|\\.)*)"\s*\)',
        header_text,
    ):
        values[m.group(1)] = (0, unescape_c_string(m.group(2)).decode("latin-1"))
    for m in re.finditer(
        r"DEF_STATIC_CONST_VAL_FLOAT\(\s*(\w+)\s*,\s*(-?[\d.eE+]+)\s*\)", header_text
    ):
        values[m.group(1)] = (1, float(m.group(2)))
    for m in re.finditer(
        r"DEF_STATIC_CONST_VAL_INT\(\s*(\w+)\s*,\s*(-?\d+)\s*\)", header_text
    ):
        values[m.group(1)] = (1, float(m.group(2)))
    return values, consts


def cart_payload(text: str, symbol: str, values: dict, consts: dict) -> bytes:
    """One tree, given the value and node tables its header defined."""
    # Upstream names the node table `<sym>_cart_nodes` but the feature table
    # `<sym>_feat_table`, i.e. without the `_cart` infix.
    feats = string_list(text, symbol.removesuffix("_cart") + "_feat_table")
    body = strip_comments(array_body(text, symbol + "_nodes"))

    order: list[str] = []
    val_index: dict[str, int] = {}
    nodes = bytearray()
    for m in re.finditer(r"\{([^{}]*)\}", body):
        parts = [p.strip() for p in m.group(1).split(",")]
        feat = int(parts[0])
        op = OPS[parts[1]]
        no_ref = parts[2]
        no_node = int(no_ref) if no_ref.lstrip("-").isdigit() else consts[no_ref]
        val_ref = parts[3]
        vm = re.search(r"&(\w+)", val_ref)
        if vm is None:  # the sentinel `0` value that terminates the table
            continue
        vname = vm.group(1)
        if vname not in val_index:
            val_index[vname] = len(order)
            order.append(vname)
        nodes += struct.pack("<BBHH", feat, op, no_node, val_index[vname])

    vals = bytearray(struct.pack("<I", len(order)))
    for vname in order:
        kind, v = values[vname]
        vals.append(kind)
        if kind == 0:
            raw = v.encode("utf-8")
            assert len(raw) < 256
            vals.append(len(raw))
            vals += raw
        else:
            vals += struct.pack("<f", v)

    out = bytearray()
    out += string_table(feats)
    out += bytes(vals)
    out += struct.pack("<I", len(nodes) // 6)
    out += bytes(nodes)
    return bytes(out)


CARTS = [
    ("phrasing", "us_phrasing_cart"),
    ("pos", "us_pos_cart"),
    ("nums", "us_nums_cart"),
    ("accent", "us_int_accent_cart"),
    ("tone", "us_int_tone_cart"),
    ("dur", "us_durz_cart"),
]


def convert_carts(src: str, out: Container) -> None:
    usdir = os.path.join(src, "lang", "usenglish")
    for name, symbol in CARTS:
        payload = convert_cart(
            os.path.join(usdir, symbol + ".c"),
            os.path.join(usdir, symbol + ".h"),
            symbol,
        )
        out.add("cart." + name, payload)


def convert_aswd(src: str, out: Container) -> None:
    text = read(os.path.join(src, "lang", "usenglish", "us_aswd.c"))
    for tag in ("P", "S"):
        prefix = f"fsm_aswd{tag}_"
        # Each transition is `#define <name> ((<state macro> * 128) + <symbol>)`
        # and each state macro is `#define <name> <index>`.
        states = {
            m.group(1): int(m.group(2))
            for m in re.finditer(rf"#define\s+({prefix}state_\d+)\s+(\d+)", text)
        }
        trans: dict[int, int] = {}
        for m in re.finditer(
            rf"#define\s+{prefix}trans_(\d+)\s+(?:\(\(({prefix}state_\d+)\s*\*\s*128\)\s*\+\s*(\d+)\)|0)",
            text,
        ):
            slot = int(m.group(1))
            if m.group(2) is None:
                trans[slot] = 0
            else:
                trans[slot] = states[m.group(2)] * 128 + int(m.group(3))
        table = [trans[i] for i in range(max(trans) + 1)]
        out.add(
            "aswd." + tag.lower(),
            struct.pack("<I", len(table)) + b"".join(struct.pack("<H", v) for v in table),
        )


# Residual encodings, matching the `codec` field of Flite's cst_sts_list. A
# voice that names no codec stores its residual as plain mu-law.
CODEC_ULAW = 0
CODEC_G721 = 1


def voice_settings(voxdir: str, voice: str) -> tuple[float, float, float, int]:
    """What the voice registers itself with, as opposed to what its tables hold.

    The pitch and rate constants and the choice of postlexical rules live in the
    registration function rather than in any data table, so they are read out of
    the C source and carried in the voice file. Keeping them there is what lets
    a second voice differ without touching any Rust.

    `fix_ah` is the one place the two voices' postlexical rules diverge: the
    8 kHz database records that vowel only as `aa`, so the voice rewrites it,
    while the 16 kHz one has both and leaves it alone. Treating that as a
    property of the language rather than of the voice mispronounces and
    mistimes every `ah` in the other voice.
    """
    text = read(os.path.join(voxdir, f"{voice}.c"))
    found = dict(
        re.findall(r'feat_set_float\s*\(\s*v->features\s*,\s*"(\w+)"\s*,\s*([-\d.]+)', text)
    )
    return (
        float(found["duration_stretch"]),
        float(found["int_f0_target_mean"]),
        float(found["int_f0_target_stddev"]),
        1 if re.search(r"fix_ah\s*\(\s*u\s*\)", text) else 0,
    )


def convert_voice(src: str, out: Container, voice: str) -> None:
    voxdir = os.path.join(src, "lang", voice)
    dip = read(os.path.join(voxdir, f"{voice}_diphone.c"))

    sts = re.search(rf"cst_sts_list\s+{voice}_sts\s*=\s*\{{(.*?)\}};", dip, re.S).group(1)
    sts = strip_comments(sts).replace("#ifdef", " ")
    # A trailing string literal names the residual codec; its absence means
    # the residual is raw mu-law and there is no ressize table either.
    codec_name = re.search(r'"([^"]*)"\s*$', sts.strip())
    codec = CODEC_G721 if codec_name and codec_name.group(1) == "g721" else CODEC_ULAW
    if codec_name and codec_name.group(1) not in ("g721",):
        sys.exit(f"unsupported residual codec {codec_name.group(1)!r} in {voice}")

    nums = re.findall(r"-?\d+\.\d+|\b\d+\b", sts)
    num_frames, num_channels, sample_rate = int(nums[-5]), int(nums[-4]), int(nums[-3])
    coeff_min, coeff_range = float(nums[-2]), float(nums[-1])
    stretch, f0_mean, f0_stddev, fix_ah = voice_settings(voxdir, voice)
    print(
        f"  {voice}: {num_frames} frames x {num_channels} ch @ {sample_rate} Hz, "
        f"{'g721' if codec else 'ulaw'} residual"
        f"{', folds ah to aa' if fix_ah else ''}"
    )

    out.add("voice.backend", b"diphone")
    entries = []
    body = strip_comments(array_body(dip, f"{voice}_index"))
    for m in re.finditer(r'\{\s*"([^"]*)"\s*,\s*(\d+)\s*,\s*(\d+)\s*,\s*(\d+)\s*\}', body):
        entries.append((m.group(1), int(m.group(2)), int(m.group(3)), int(m.group(4))))
    print(f"  {voice}: {len(entries)} diphones")

    index = bytearray(struct.pack("<I", len(entries)))
    for name, start, pb, end in entries:
        raw = name.encode("ascii")
        index.append(len(raw))
        index += raw
        index += struct.pack("<HBB", start, pb, end)

    # Each generated array carries one trailing sentinel element; the residual
    # offset table is the exception, holding num_frames + 1 real offsets so the
    # last frame's extent is known.
    lpc = int_array(read(os.path.join(voxdir, f"{voice}_lpc.c")), f"{voice}_lpc")
    lpc = lpc[: num_frames * num_channels]
    resi = int_array(read(os.path.join(voxdir, f"{voice}_residx.c")), f"{voice}_resi")
    resi = resi[: num_frames + 1]
    res = int_array(read(os.path.join(voxdir, f"{voice}_res.c")), f"{voice}_res")
    res = res[: resi[-1]]
    assert len(lpc) == num_frames * num_channels and len(res) == resi[-1]

    out.add(
        "sts.header",
        struct.pack(
            "<IIIffIfffI",
            num_frames,
            num_channels,
            sample_rate,
            coeff_min,
            coeff_range,
            codec,
            stretch,
            f0_mean,
            f0_stddev,
            fix_ah,
        ),
    )
    out.add("sts.lpc", struct.pack(f"<{len(lpc)}H", *lpc))
    out.add("sts.resoffs", struct.pack(f"<{len(resi)}I", *resi))
    # Only a compressed residual needs a decoded-length table: for raw mu-law
    # the offsets already give it, and the voice ships none.
    if codec != CODEC_ULAW:
        ressize = int_array(read(os.path.join(voxdir, f"{voice}_ressize.c")), f"{voice}_ressize")
        out.add("sts.ressize", bytes(ressize[:num_frames]))
    out.add("sts.res", bytes(res))
    out.add("dip.index", bytes(index))


def dump_rust_tables(src: str) -> None:
    """Print the small tables in a form convenient to paste into Rust source."""
    usdir = os.path.join(src, "lang", "usenglish")

    text = read(os.path.join(usdir, "us_phoneset.c"))
    names = string_list(text, "us_phonenames")
    featvals = [
        unescape_c_string(m.group(2)).decode("latin-1")
        for m in re.finditer(r'DEF_STATIC_CONST_VAL_STRING\((featval_\d+),"((?:[^"\\]|\\.)*)"\)', text)
    ]
    fv = {}
    for m in re.finditer(r"static const int (us_fv_\d+)\[\]\s*=\s*\{([^}]*)\}", text):
        fv[m.group(1)] = [int(x) for x in re.findall(r"-?\d+", m.group(2))[:8]]
    order = re.findall(r"us_fv_\d+", array_body(text, "us_fvtable"))
    print("\n// --- phoneset ---")
    for name, key in zip(names, order):
        vals = ", ".join('"%s"' % featvals[i] for i in fv[key])
        print(f'    p("{name}", [{vals}]),')

    text = read(os.path.join(usdir, "us_dur_stats.c"))
    print("\n// --- duration stats ---")
    for m in re.finditer(r'\{\s*"(\w+)"\s*,\s*([\d.]+)\s*,\s*([\d.]+)\s*\}', text):
        print(f'    ("{m.group(1)}", {m.group(2)}, {m.group(3)}),')

    text = read(os.path.join(usdir, "us_gpos.c"))
    print("\n// --- gpos ---")
    literals = {
        m.group(1): unescape_c_string(m.group(2)).decode("latin-1")
        for m in re.finditer(r'DEF_STATIC_CONST_VAL_STRING\((gpos_\w+),"((?:[^"\\]|\\.)*)"\)', text)
    }
    for m in re.finditer(r"static const cst_val \* const (gpos_\w+_list)\[\]\s*=\s*\{([^}]*)\}", text):
        words = [literals[w] for w in re.findall(r"&(gpos_\w+)", m.group(2))]
        klass, rest = words[0], sorted(set(words[1:]))
        print(f'    ("{klass}", &{rest!r}),'.replace("'", '"'))

    text = read(os.path.join(usdir, "us_f0lr.c"))
    print("\n// --- f0 lr terms ---")
    for m in re.finditer(
        r'\{\s*"([^"]*)"\s*,\s*(-?[\d.]+)\s*,\s*(-?[\d.]+)\s*,\s*(-?[\d.]+)\s*,\s*(0|"[^"]*")\s*\}', text
    ):
        typ = "None" if m.group(5) == "0" else f'Some("{m.group(5)[1:-1]}")'
        print(f'    ("{m.group(1)}", {m.group(2)}, {m.group(3)}, {m.group(4)}, {typ}),')


# ---------------------------------------------------------------------------
# Clustergen voices.
#
# Upstream ships three of these in-tree (lang/cmu_us_{awb,rms,slt}) as
# generated C arrays, exactly like the diphone data, so they convert the same
# way and need no external .flitevox file.  docs/clustergen-voice-data.md
# records that decision and where the data comes from.
# ---------------------------------------------------------------------------

# Values of cst_cg_db.model_shape, from include/cst_cg.h.  Only the first is
# implemented: it is what all three in-tree voices use, and the quantised
# shapes would need their q-tables carried too.
CG_SHAPE_BASE_MINRANGE = 1
CG_SHAPE_QUANTIZED_PARAMS = 2
CG_SHAPE_QUANTIZED_PARAMS_41 = 3

CG_SHAPES = {
    "CST_CG_MODEL_SHAPE_BASE_MINRANGE": CG_SHAPE_BASE_MINRANGE,
    "CST_CG_MODEL_SHAPE_QUANTIZED_PARAMS": CG_SHAPE_QUANTIZED_PARAMS,
    "CST_CG_MODEL_SHAPE_QUANTIZED_PARAMS_41": CG_SHAPE_QUANTIZED_PARAMS_41,
}


def float_array(text: str, name: str) -> list[float]:
    body = strip_comments(array_body(text, name))
    return [float(tok) for tok in re.findall(r"-?\d+(?:\.\d+)?(?:[eE][-+]?\d+)?", body)]


def tree_list(symbols: list[str], text: str, values: dict, consts: dict) -> bytes:
    """A run of CART trees as one section: count, then each length-prefixed."""
    out = bytearray(struct.pack("<I", len(symbols)))
    for symbol in symbols:
        payload = cart_payload(text, symbol, values, consts)
        out += struct.pack("<I", len(payload))
        out += payload
    return bytes(out)


def cg_phone_states(path: str) -> bytes:
    """The phone-to-HMM-state map: `{"aa", "aa_1", "aa_2", "aa_3", 0}` each.

    The first string is the phone and the rest are its state names, which is
    the layout cst_cg.c walks when it builds the HMMstate relation.
    """
    text = strip_comments(read(path))
    entries = []
    for m in re.finditer(r"_ps\[\]\s*=\s*\{(.*?)\}", text, re.S):
        names = [g.group(1) for g in STRING_LITERAL.finditer(m.group(1))]
        if names:
            entries.append(names)
    out = bytearray(struct.pack("<I", len(entries)))
    for names in entries:
        assert len(names) < 256, names
        out.append(len(names))
        for name in names:
            raw = name.encode("utf-8")
            assert len(raw) < 256, name
            out.append(len(raw))
            out += raw
    return bytes(out)


def cg_dur_stats(path: str) -> bytes:
    """Per-state duration mean and stddev, in the order the table lists them.

    Order matters: cg_state_duration scans this table linearly and falls back
    to entry 0 for a name it does not find, so entry 0 has to stay entry 0.
    """
    text = read(path)
    literals = {
        m.group(1): (m.group(2), float(m.group(3)), float(m.group(4)))
        for m in re.finditer(
            r'static const dur_stat\s+(\w+)\s*=\s*\{\s*"([^"]*)"\s*,\s*([\d.eE+-]+)\s*,'
            r"\s*([\d.eE+-]+)\s*\}",
            text,
        )
    }
    order = re.findall(r"&(\w+)", array_body(text, "dur_stats"))
    out = bytearray(struct.pack("<I", len(order)))
    for name in order:
        phone, mean, stddev = literals[name]
        raw = phone.encode("utf-8")
        assert len(raw) < 256, phone
        out.append(len(raw))
        out += raw
        out += struct.pack("<ff", mean, stddev)
    return bytes(out)


def cg_model_vectors(path: str, num_frames: int, num_channels: int) -> bytes:
    """The parameter store: one unsigned-short vector per cluster frame.

    Read in one pass rather than one regex per frame, because this file is
    eight megabytes of C.
    """
    text = read(path)
    frames: dict[int, list[int]] = {}
    for m in re.finditer(r"param_frame_(\d+)\[\]\s*=\s*\{([^}]*)\}", text):
        frames[int(m.group(1))] = [int(t) for t in re.findall(r"\d+", m.group(2))]
    # The model_vectors array fixes the order; it happens to be frame_0 upward,
    # but reading it rather than assuming that is cheap.
    order = [int(m) for m in re.findall(r"param_frame_(\d+)", array_body(text, "model_vectors"))]
    if len(order) != num_frames:
        sys.exit(f"{path}: {len(order)} frames, header says {num_frames}")
    out = bytearray()
    for index in order:
        vector = frames[index]
        if len(vector) != num_channels:
            sys.exit(f"{path}: frame {index} has {len(vector)} channels, not {num_channels}")
        out += struct.pack(f"<{num_channels}H", *vector)
    return bytes(out)


def cg_define(text: str, name: str) -> int:
    m = re.search(rf"#define\s+{re.escape(name)}\s+(\d+)", text)
    if m is None:
        raise KeyError(name)
    return int(m.group(1))


def cg_int(text: str, field: str) -> int:
    """A struct field that is either a literal or the name of a #define."""
    field = field.strip()
    return int(field) if field.lstrip("-").isdigit() else cg_define(text, field)


def cg_db_fields(text: str, voice: str) -> list[str]:
    """The initialiser of the cst_cg_db struct, one field per element.

    Positional, because that is how the struct is written: upstream's own
    comment on cst_cg_db says only to append to it, so the order is stable.
    """
    body = array_body_braced(text, rf"const cst_cg_db\s+{voice}_cg_db\s*=\s*\{{")
    return [f.strip() for f in split_top_level(strip_comments(body))]


def array_body_braced(text: str, pattern: str) -> str:
    """`array_body` for a declaration matched by a regex rather than a name."""
    m = re.search(pattern, text)
    if m is None:
        raise KeyError(pattern)
    start = m.end()
    depth = 1
    i = start
    while depth:
        if text[i] == "{":
            depth += 1
        elif text[i] == "}":
            depth -= 1
        i += 1
    return text[start : i - 1]


def split_top_level(body: str) -> list[str]:
    """Split on commas that are not inside brackets, quotes or braces."""
    parts: list[str] = []
    depth = 0
    current = ""
    for c in body:
        if c in "([{":
            depth += 1
        elif c in ")]}":
            depth -= 1
        if c == "," and depth == 0:
            parts.append(current)
            current = ""
        else:
            current += c
    if current.strip():
        parts.append(current)
    return parts


def convert_clustergen_voice(src: str, out: Container, voice: str) -> None:
    voxdir = os.path.join(src, "lang", voice)
    db = read(os.path.join(voxdir, f"{voice}_cg.c"))
    fields = cg_db_fields(db, voice)

    # Field positions in cst_cg_db, from include/cst_cg.h.  Named rather than
    # indexed inline so that a struct change shows up here as one edit.
    (
        _name, _types, _num_types, f_sample_rate, f_f0_mean, f_f0_stddev,
        _num_f0_models, _f0_trees, _num_param_models, _param_trees,
        _spam_accent_tree, _spam_phrase_tree, _num_channels, _num_frames,
        _model_vectors, _spam_num_channels, _spam_num_frames, _spam_vectors,
        _model_min, _model_range, _qtable, f_model_shape, f_frame_advance,
        _num_dur_models, _dur_stats, _dur_cart, _phone_states, f_do_mlpg,
        _dynwin, f_dynwinsize, f_mlsa_alpha, f_mlsa_beta, f_multimodel,
        f_mixed_excitation, f_me_num, f_me_order, _me_h, f_spamf0, f_gain,
    ) = fields[:39]

    shape = CG_SHAPES[f_model_shape.strip()]
    if shape != CG_SHAPE_BASE_MINRANGE:
        sys.exit(
            f"{voice}: model_shape {f_model_shape.strip()} is quantised and its "
            "q-tables are not converted"
        )
    if cg_int(db, f_multimodel):
        sys.exit(f"{voice}: multimodel voices are not converted")
    if any(
        cg_int(db, f) != 1 for f in (_num_param_models, _num_f0_models, _num_dur_models)
    ):
        sys.exit(f"{voice}: only single-model voices are converted")
    if cg_int(db, f_spamf0):
        sys.exit(f"{voice}: the SPAM F0 model is not implemented")

    types = string_list(db, f"{voice}_types")
    num_channels = cg_define(db, f"{voice}_single_num_channels")
    num_frames = cg_define(db, f"{voice}_single_num_frames")
    model_min = float_array(db, f"{voice}_model_min")
    model_range = float_array(db, f"{voice}_model_range")
    dynwin = float_array(db, f"{voice}_dynwin")
    dynwinsize = cg_int(db, f_dynwinsize)
    me_num, me_order = cg_int(db, f_me_num), cg_int(db, f_me_order)
    me_filters = [float_array(db, f"{voice}_me_filter_{i}") for i in range(me_num)]

    for name, got, want in (
        ("model_min", len(model_min), num_channels),
        ("model_range", len(model_range), num_channels),
        ("dynwin", len(dynwin), dynwinsize),
    ):
        if got != want:
            sys.exit(f"{voice}: {name} has {got} entries, expected {want}")
    for i, f in enumerate(me_filters):
        if len(f) != me_order:
            sys.exit(f"{voice}: me_filter_{i} has {len(f)} taps, expected {me_order}")

    print(
        f"  {voice}: clustergen, {num_frames} frames x {num_channels} ch @ "
        f"{cg_int(db, f_sample_rate)} Hz, {len(types)} state types"
        f"{', mixed excitation' if cg_int(db, f_mixed_excitation) else ''}"
        f"{', mlpg' if cg_int(db, f_do_mlpg) else ''}"
    )

    out.add("voice.backend", b"clustergen")
    out.add(
        "cg.header",
        struct.pack(
            "<IffffffIIIIIIIIII",
            cg_int(db, f_sample_rate),
            float(f_f0_mean),
            float(f_f0_stddev),
            float(f_frame_advance),
            float(f_mlsa_alpha),
            float(f_mlsa_beta),
            float(f_gain),
            len(types),
            num_channels,
            num_frames,
            shape,
            cg_int(db, f_do_mlpg),
            cg_int(db, f_mixed_excitation),
            me_num,
            me_order,
            dynwinsize,
            cg_int(db, f_spamf0),
        ),
    )
    out.add("cg.types", string_table(types))
    out.add("cg.modelmin", struct.pack(f"<{num_channels}f", *model_min))
    out.add("cg.modelrange", struct.pack(f"<{num_channels}f", *model_range))
    out.add("cg.dynwin", struct.pack(f"<{dynwinsize}f", *dynwin))
    # The mixed-excitation filters are doubles upstream and are used as
    # doubles, so they are not narrowed on the way through.
    out.add(
        "cg.mefilters",
        b"".join(struct.pack(f"<{me_order}d", *f) for f in me_filters),
    )
    out.add(
        "cg.params",
        cg_model_vectors(
            os.path.join(voxdir, f"{voice}_cg_single_params.c"), num_frames, num_channels
        ),
    )
    out.add(
        "cg.phonestates", cg_phone_states(os.path.join(voxdir, f"{voice}_cg_phonestate.c"))
    )
    out.add("cg.durstats", cg_dur_stats(os.path.join(voxdir, f"{voice}_cg_durmodel.c")))

    dur_c = os.path.join(voxdir, f"{voice}_cg_durmodel.c")
    dur_h = os.path.join(voxdir, f"{voice}_cg_durmodel.h")
    out.add("cg.durtree", convert_cart(dur_c, dur_h, f"{voice}_dur_cart"))

    # One tree per state type, in the order `cg.types` lists them: that
    # parallelism is what lets the backend index trees by type.
    for section, stem, prefix in (
        ("cg.f0trees", "cg_f0_trees", f"{voice}_f0_"),
        ("cg.mceptrees", "cg_single_mcep_trees", f"{voice}_single_mcep_"),
    ):
        text = read(os.path.join(voxdir, f"{voice}_{stem}.c"))
        values, consts = cart_tables(read(os.path.join(voxdir, f"{voice}_{stem}.h")))
        symbols = [f"{prefix}{t}_cart" for t in types]
        out.add(section, tree_list(symbols, text, values, consts))


def main() -> int:
    if len(sys.argv) < 2:
        print(__doc__)
        return 2
    src = sys.argv[1]
    outdir = sys.argv[2] if len(sys.argv) > 2 else os.path.join(os.path.dirname(__file__), "..", "data")
    os.makedirs(outdir, exist_ok=True)

    if os.environ.get("DUMP_TABLES"):
        dump_rust_tables(src)
        return 0

    print("language data:")
    lang = Container()
    convert_lexicon(src, lang)
    convert_lts(src, lang)
    convert_carts(src, lang)
    convert_aswd(src, lang)
    lang.write(os.path.join(outdir, "en_us.dat"))

    for name, backend in VOICES:
        print("voice data:")
        voice = Container()
        if backend == "clustergen":
            convert_clustergen_voice(src, voice, name)
        else:
            convert_voice(src, voice, name)
        voice.write(os.path.join(outdir, f"{name}.dat"))
    return 0


if __name__ == "__main__":
    sys.exit(main())
