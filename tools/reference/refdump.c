/* Reference driver: dump the linguistic structure upstream Flite builds.

   Output is in the same order and format as `cargo run --example analysis`, so
   a divergence can be bisected by diffing the two: phones and their end times
   first, then the syllable-level intonation decisions, then the pitch contour.
   Whichever section differs first names the stage that broke.

   flite_synth_text synthesises the argument as one utterance, so only pass a
   single sentence. Multi-sentence input is not comparable here, because
   flite-rs splits it; use reffile and compare the audio for that. */

#include <stdio.h>
#include <string.h>
#include "flite.h"
#include "cst_utterance.h"
#include "cst_relation.h"
#include "cst_item.h"
#include "cst_sts.h"
#include "cst_sigpr.h"

#ifdef _WIN32
#include <io.h>
#include <fcntl.h>
#endif

cst_voice *register_cmu_us_kal(const char *voxdir);
cst_voice *register_cmu_us_kal16(const char *voxdir);

int main(int argc, char **argv)
{
    cst_voice *v;
    cst_utterance *u;
    cst_item *i;
    int arg;
    int phones = 0, units = 0;
    const char *voice = "kal";

#ifdef _WIN32
    /* Emit LF rather than CRLF, so this diffs against the analysis example
       instead of differing on every single line. */
    _setmode(_fileno(stdout), _O_BINARY);
#endif

    if (argc < 2)
    {
        fprintf(stderr, "usage: refdump [-p] [-u] [voice=NAME] \"one sentence\"\n");
        return 1;
    }
    /* -p reads the argument as phones, which is a different pipeline: no
       lexicon, no intonation, and a flat contour. -u adds the selected units
       and the output pitch marks, which is where a divergence goes when the
       linguistic sections agree. */
    for (arg = 1; arg < argc - 1; arg++)
    {
        if (cst_streq(argv[arg], "-p"))
            phones = 1;
        else if (cst_streq(argv[arg], "-u"))
            units = 1;
        else if (strncmp(argv[arg], "voice=", 6) == 0)
            voice = argv[arg] + 6;
    }

    flite_init();
    v = cst_streq(voice, "kal16") ? register_cmu_us_kal16(NULL)
                                  : register_cmu_us_kal(NULL);
    if (!v)
    {
        fprintf(stderr, "refdump: could not register %s\n", voice);
        return 1;
    }
    u = phones ? flite_synth_phones(argv[argc - 1], v)
               : flite_synth_text(argv[argc - 1], v);

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

    if (units)
    {
        cst_lpcres *lr;
        int k;

        printf("UNITS\n");
        for (i = relation_head(utt_relation(u, "Unit")); i; i = item_next(i))
            printf("%s %d %d %d\n", item_feat_string(i, "name"),
                   item_feat_int(i, "unit_start"), item_feat_int(i, "unit_end"),
                   item_feat_int(i, "target_end"));

        lr = val_lpcres(utt_feat_val(u, "target_lpcres"));
        printf("PITCHMARKS %d\n", lr->num_frames);
        for (k = 0; k < lr->num_frames; k++)
            printf("%d %d\n", lr->times[k], lr->sizes[k]);
    }

    return 0;
}
