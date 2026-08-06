/* Reference driver: text file in, WAV out, using upstream Flite.

   flite_file_to_speech splits the input into sentences and synthesises each
   one separately, which is what flite-rs does. flite_text_to_speech treats the
   whole input as a single utterance instead, so it only agrees with flite-rs
   on single-sentence input and is the wrong entry point for this comparison.
   Getting that wrong produces differences that look like synthesis bugs.

   Trailing NAME=VALUE arguments set voice features, and voice=NAME picks which
   voice to register. Both are how the paths a voice does not ask for by itself
   get exercised. */

#include <stdio.h>
#include <string.h>
#include "flite.h"

cst_voice *register_cmu_us_kal(const char *voxdir);
cst_voice *register_cmu_us_kal16(const char *voxdir);

int main(int argc, char **argv)
{
    cst_voice *v;
    const char *voice = "kal";
    int i;

    if (argc < 3)
    {
        fprintf(stderr, "usage: reffile in.txt out.wav [NAME=VALUE ...]\n");
        return 1;
    }
    flite_init();

    for (i = 3; i < argc; i++)
        if (strncmp(argv[i], "voice=", 6) == 0)
            voice = argv[i] + 6;

    if (strcmp(voice, "kal16") == 0)
        v = register_cmu_us_kal16(NULL);
    else
        v = register_cmu_us_kal(NULL);
    if (!v)
    {
        fprintf(stderr, "reffile: could not register %s\n", voice);
        return 1;
    }

    for (i = 3; i < argc; i++)
    {
        char *equals = strchr(argv[i], '=');
        if (!equals || strncmp(argv[i], "voice=", 6) == 0)
            continue;
        *equals = '\0';
        flite_feat_set_string(v->features, argv[i], equals + 1);
    }

    flite_file_to_speech(argv[1], v, argv[2]);
    return 0;
}
