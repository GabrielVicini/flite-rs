/* Reference driver: text file in, WAV out, using upstream Flite.

   flite_file_to_speech splits the input into sentences and synthesises each
   one separately, which is what flite-rs does. flite_text_to_speech treats the
   whole input as a single utterance instead, so it only agrees with flite-rs
   on single-sentence input and is the wrong entry point for this comparison.
   Getting that wrong produces differences that look like synthesis bugs. */

#include <stdio.h>
#include "flite.h"

cst_voice *register_cmu_us_kal(const char *voxdir);

int main(int argc, char **argv)
{
    cst_voice *v;

    if (argc < 3)
    {
        fprintf(stderr, "usage: reffile in.txt out.wav [join_type] [resynth_type]\n");
        return 1;
    }
    flite_init();
    v = register_cmu_us_kal(NULL);
    if (!v)
    {
        fprintf(stderr, "reffile: could not register cmu_us_kal\n");
        return 1;
    }
    /* The voice sets both of these itself, so overriding them is the only way
       to reach the join and resynthesis paths it does not ask for. */
    if (argc > 3)
        flite_feat_set_string(v->features, "join_type", argv[3]);
    if (argc > 4)
        flite_feat_set_string(v->features, "resynth_type", argv[4]);
    flite_file_to_speech(argv[1], v, argv[2]);
    return 0;
}
