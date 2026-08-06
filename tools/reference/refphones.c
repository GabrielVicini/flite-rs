/* Reference driver: synthesise a phone string rather than text.

   This is the path that skips the lexicon and the intonation model, so the
   only shared machinery with reffile is the duration model and the waveform
   stage. A phone string is always one utterance, so unlike text there is
   nothing here about sentence splitting. */

#include <stdio.h>
#include "flite.h"

cst_voice *register_cmu_us_kal(const char *voxdir);

int main(int argc, char **argv)
{
    cst_voice *v;

    if (argc < 3)
    {
        fprintf(stderr, "usage: refphones \"pau hh ax l ow pau\" out.wav\n");
        return 1;
    }
    flite_init();
    v = register_cmu_us_kal(NULL);
    if (!v)
    {
        fprintf(stderr, "refphones: could not register cmu_us_kal\n");
        return 1;
    }
    flite_phones_to_speech(argv[1], v, argv[2]);
    return 0;
}
