#!/usr/bin/env python3
"""Convert Flite's C data tables into flite-rs binary data files.

This script is a *build-time* tool, not part of the shipped library.  It reads
the upstream Flite distribution's generated C arrays (lexicon, letter-to-sound
model, CART trees, and the cmu_us_kal diphone voice) and repacks them into the
two container files that flite-rs embeds:

    data/en_us.dat        language data (lexicon, LTS, CARTs, aswd FSMs)
    data/cmu_us_kal.dat   voice data (diphone index, LPC frames, residuals)

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
    header = read(src_h)
    consts: dict[str, int] = {}
    for m in re.finditer(r"#define\s+(CTNODE\w+)\s+(\d+)", header):
        consts[m.group(1)] = int(m.group(2))

    values: dict[str, tuple[int, object]] = {}
    for m in re.finditer(r'DEF_STATIC_CONST_VAL_STRING\(\s*(\w+)\s*,\s*"((?:[^"\\]|\\.)*)"\s*\)', header):
        values[m.group(1)] = (0, unescape_c_string(m.group(2)).decode("latin-1"))
    for m in re.finditer(r"DEF_STATIC_CONST_VAL_FLOAT\(\s*(\w+)\s*,\s*(-?[\d.eE+]+)\s*\)", header):
        values[m.group(1)] = (1, float(m.group(2)))
    for m in re.finditer(r"DEF_STATIC_CONST_VAL_INT\(\s*(\w+)\s*,\s*(-?\d+)\s*\)", header):
        values[m.group(1)] = (1, float(m.group(2)))

    text = read(src_c)
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


def convert_voice(src: str, out: Container) -> None:
    voxdir = os.path.join(src, "lang", "cmu_us_kal")
    dip = read(os.path.join(voxdir, "cmu_us_kal_diphone.c"))

    sts = re.search(r"cst_sts_list\s+cmu_us_kal_sts\s*=\s*\{(.*?)\};", dip, re.S).group(1)
    nums = re.findall(r"-?\d+\.\d+|\b\d+\b", strip_comments(sts).replace("#ifdef", " "))
    num_frames, num_channels, sample_rate = int(nums[-5]), int(nums[-4]), int(nums[-3])
    coeff_min, coeff_range = float(nums[-2]), float(nums[-1])
    print(f"  voice: {num_frames} frames x {num_channels} ch @ {sample_rate} Hz")

    entries = []
    body = strip_comments(array_body(dip, "cmu_us_kal_index"))
    for m in re.finditer(r'\{\s*"([^"]*)"\s*,\s*(\d+)\s*,\s*(\d+)\s*,\s*(\d+)\s*\}', body):
        entries.append((m.group(1), int(m.group(2)), int(m.group(3)), int(m.group(4))))
    print(f"  voice: {len(entries)} diphones")

    index = bytearray(struct.pack("<I", len(entries)))
    for name, start, pb, end in entries:
        raw = name.encode("ascii")
        index.append(len(raw))
        index += raw
        index += struct.pack("<HBB", start, pb, end)

    # Each generated array carries one trailing sentinel element; the residual
    # offset table is the exception, holding num_frames + 1 real offsets so the
    # last frame's extent is known.
    lpc = int_array(read(os.path.join(voxdir, "cmu_us_kal_lpc.c")), "cmu_us_kal_lpc")
    lpc = lpc[: num_frames * num_channels]
    resi = int_array(read(os.path.join(voxdir, "cmu_us_kal_residx.c")), "cmu_us_kal_resi")
    resi = resi[: num_frames + 1]
    ressize = int_array(read(os.path.join(voxdir, "cmu_us_kal_ressize.c")), "cmu_us_kal_ressize")
    ressize = ressize[:num_frames]
    res = int_array(read(os.path.join(voxdir, "cmu_us_kal_res.c")), "cmu_us_kal_res")
    res = res[: resi[-1]]
    assert len(lpc) == num_frames * num_channels and len(res) == resi[-1]

    out.add(
        "sts.header",
        struct.pack("<IIIff", num_frames, num_channels, sample_rate, coeff_min, coeff_range),
    )
    out.add("sts.lpc", struct.pack(f"<{len(lpc)}H", *lpc))
    out.add("sts.resoffs", struct.pack(f"<{len(resi)}I", *resi))
    out.add("sts.ressize", bytes(ressize))
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

    print("voice data:")
    voice = Container()
    convert_voice(src, voice)
    voice.write(os.path.join(outdir, "cmu_us_kal.dat"))
    return 0


if __name__ == "__main__":
    sys.exit(main())
