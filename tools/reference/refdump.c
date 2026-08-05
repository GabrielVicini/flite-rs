/* Reference driver: dump the linguistic structure upstream Flite builds.

   Output is in the same order and format as `cargo run --example analysis`, so
   a divergence can be bisected by diffing the two: phones and their end times
   first, then the syllable-level intonation decisions, then the pitch contour.
   Whichever section differs first names the stage that broke.

   flite_synth_text synthesises the argument as one utterance, so only pass a
   single sentence. Multi-sentence input is not comparable here, because
   flite-rs splits it; use reffile and compare the audio for that. */

#include <stdio.h>
#include "flite.h"
#include "cst_utterance.h"
#include "cst_relation.h"
#include "cst_item.h"

#ifdef _WIN32
#include <io.h>
#include <fcntl.h>
#endif

cst_voice *register_cmu_us_kal(const char *voxdir);

int main(int argc, char **argv)
{
    cst_voice *v;
    cst_utterance *u;
    cst_item *i;

#ifdef _WIN32
    /* Emit LF rather than CRLF, so this diffs against the analysis example
       instead of differing on every single line. */
    _setmode(_fileno(stdout), _O_BINARY);
#endif

    if (argc < 2)
    {
        fprintf(stderr, "usage: refdump \"one sentence\"\n");
        return 1;
    }
    flite_init();
    v = register_cmu_us_kal(NULL);
    if (!v)
    {
        fprintf(stderr, "refdump: could not register cmu_us_kal\n");
        return 1;
    }
    u = flite_synth_text(argv[1], v);

    printf("SEGMENTS\n");
    for (i = relation_head(utt_relation(u, "Segment")); i; i = item_next(i))
        printf("%s %.6f\n", item_name(i), item_feat_float(i, "end"));

    printf("SYLLABLES\n");
    for (i = relation_head(utt_relation(u, "Syllable")); i; i = item_next(i))
        printf("%s stress=%s accent=%s endtone=%s\n",
               ffeature_string(i, "R:SylStructure.parent.name"),
               ffeature_string(i, "stress"),
               ffeature_string(i, "accent"),
               ffeature_string(i, "endtone"));

    printf("TARGETS\n");
    for (i = relation_head(utt_relation(u, "Target")); i; i = item_next(i))
        printf("%.6f %.4f\n", item_feat_float(i, "pos"), item_feat_float(i, "f0"));

    return 0;
}
