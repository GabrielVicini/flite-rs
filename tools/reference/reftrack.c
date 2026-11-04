/* Reference driver: dump a Clustergen voice's parameter trajectories.

   This exists because the Clustergen waveform is not the whole story. Stages
   one to four of CG synthesis — HMM states, frame layout, tree prediction and
   MLPG smoothing — are deterministic, and comparing them directly says
   something much sharper than comparing audio: a divergence lands on a named
   stage instead of on a sample number. Comparing only the waveform would also
   leave the smoothing untested except through a vocoder that is itself the
   least portable part of the system.

   Output is plain text, one section per stage, which flite-rs reproduces for
   `cargo run --example cgtrack`:

     FRAMES <count>
     SEGMENTS <count>                 phone name and end time, one per line
     STATES <count>                   HMM state name, statepos, end time
     PARAM <frames> <channels>        the predicted track, F0 then the model
                                      means and standard deviations, after F0
                                      splining and rescaling
     STR <frames> <channels>          per-band voicing strengths
     MLPG <frames> <channels>         the same track after MLPG smoothing

   With -f it instead prints, for every HMM state, every feature the duration
   tree consults and that feature's value. That is the last resort the README
   prescribes when the end times differ and nothing explains why: diffing two
   of these names the feature, and the feature names the bug.

   Floats print with %.9g, which round-trips a single-precision value exactly,
   so a difference in the last bit still shows as a difference in the text.

   mlpg() is called from here rather than being read out of the utterance,
   because cg_resynth deletes the smoothed track as soon as it has used it.
   That it is callable at all is why this driver can isolate the smoothing. */

#include <stdio.h>
#include <string.h>
#include "flite.h"
#include "cst_utterance.h"
#include "cst_relation.h"
#include "cst_item.h"
#include "cst_track.h"
#include "cst_features.h"
#include "cst_cg.h"
#include "cst_ffeatures.h"

#ifdef _WIN32
#include <io.h>
#include <fcntl.h>
#endif

cst_voice *register_cmu_us_slt(const char *voxdir);

/* Every feature the duration tree asks, for every state, with its value.
   The tree carries its own feature table, so this needs no list of paths. */
static void print_features(cst_utterance *u, const cst_cart *tree)
{
    cst_item *item;
    int i, states;

    for (states = 0, item = utt_rel_head(u, "HMMstate"); item; item = item_next(item))
        states++;
    for (i = 0; tree->feat_table[i]; i++)
        ;
    printf("FEATURES %d %d\n", states, i);
    for (i = 0; tree->feat_table[i]; i++)
        printf("%s\n", tree->feat_table[i]);
    for (item = utt_rel_head(u, "HMMstate"); item; item = item_next(item))
    {
        printf("%s", item_feat_string(item, "name"));
        for (i = 0; tree->feat_table[i]; i++)
        {
            /* A path that runs off the end answers with no value at all, not
               with the string "0", so this cannot go straight to %s. */
            const cst_val *v = ffeature(item, tree->feat_table[i]);
            if (v == NULL)
                printf("\t<none>");
            else if (CST_VAL_TYPE(v) == CST_VAL_TYPE_STRING)
                printf("\t%s", val_string(v));
            else if (CST_VAL_TYPE(v) == CST_VAL_TYPE_INT)
                printf("\t%d", val_int(v));
            else
                printf("\t%.9g", val_float(v));
        }
        printf("\n");
    }
}

static void print_track(const char *name, const cst_track *t)
{
    int i, j;

    if (t == NULL)
    {
        printf("%s 0 0\n", name);
        return;
    }
    printf("%s %d %d\n", name, t->num_frames, t->num_channels);
    for (i = 0; i < t->num_frames; i++)
    {
        for (j = 0; j < t->num_channels; j++)
            printf("%s%.9g", j ? " " : "", t->frames[i][j]);
        printf("\n");
    }
}

int main(int argc, char **argv)
{
    cst_voice *v;
    cst_utterance *u;
    cst_item *item;
    cst_track *param_track, *str_track, *smoothed;
    cst_cg_db *cg_db;
    int count;
    int features = 0;

#ifdef _WIN32
    /* LF rather than CRLF, so this diffs against the Rust side instead of
       differing on every line. */
    _setmode(_fileno(stdout), _O_BINARY);
#endif

    if (argc < 2)
    {
        fprintf(stderr, "usage: reftrack [-f] \"one sentence\"\n");
        return 1;
    }
    for (count = 1; count < argc - 1; count++)
        if (cst_streq(argv[count], "-f"))
            features = 1;
    flite_init();
    v = register_cmu_us_slt(NULL);
    if (!v)
    {
        fprintf(stderr, "reftrack: could not register slt\n");
        return 1;
    }

    u = flite_synth_text(argv[argc - 1], v);
    if (!u)
    {
        fprintf(stderr, "reftrack: synthesis failed\n");
        return 1;
    }

    cg_db = val_cg_db(utt_feat_val(u, "cg_db"));

    if (features)
    {
        print_features(u, cg_db->dur_cart[0]);
        delete_utterance(u);
        return 0;
    }

    printf("FRAMES %d\n", utt_feat_int(u, "param_track_num_frames"));

    for (count = 0, item = utt_rel_head(u, "Segment"); item; item = item_next(item))
        count++;
    printf("SEGMENTS %d\n", count);
    for (item = utt_rel_head(u, "Segment"); item; item = item_next(item))
        printf("%s %.9g\n", item_feat_string(item, "name"),
               item_feat_float(item, "end"));

    for (count = 0, item = utt_rel_head(u, "HMMstate"); item; item = item_next(item))
        count++;
    printf("STATES %d\n", count);
    for (item = utt_rel_head(u, "HMMstate"); item; item = item_next(item))
        printf("%s %d %.9g\n", item_feat_string(item, "name"),
               item_feat_int(item, "statepos"), item_feat_float(item, "end"));

    param_track = val_track(utt_feat_val(u, "param_track"));
    print_track("PARAM", param_track);

    str_track = feat_present(u->features, "str_track")
                    ? val_track(utt_feat_val(u, "str_track"))
                    : NULL;
    print_track("STR", str_track);

    if (cg_db->do_mlpg)
    {
        smoothed = mlpg(param_track, cg_db);
        print_track("MLPG", smoothed);
        delete_track(smoothed);
    }
    else
        printf("MLPG 0 0\n");

    delete_utterance(u);
    return 0;
}
