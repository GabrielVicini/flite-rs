/* Pin the reference's pseudo-random source.

   Upstream's MLSA vocoder draws its unvoiced excitation from the C library's
   rand():

       static double plus_or_minus_one()
       {
           / * not sure rand() is portable * /
           if (rand() > RAND_MAX/2.0)
               return 1.0;
           else
               return -1.0;
       }

   Its comment is right. rand() is not specified beyond its range, srand is
   never called, and with mixed excitation — which every Clustergen voice
   upstream ships in-tree asks for — this is consumed on every output sample.
   So a CG waveform is a function of whichever C library built Flite: glibc's
   rand() is the TYPE_3 additive-feedback generator of random(), MSVC's is a
   32-bit LCG, and they agree on nothing.

   That is the same class of problem as -ffp-contract=off, and gets the same
   answer: an unspecified property of the C environment is pinned so that the
   reference holds still, rather than being inherited from the compiler that
   happened to build it. What it is pinned to is glibc's rand(), because that
   is what upstream is overwhelmingly built and distributed with and what the
   reference build in CI uses.

   The consequence is worth being precise about. On a glibc host this file
   reproduces exactly the sequence the real rand() would have returned, so the
   reference still behaves as unmodified upstream and flite-rs is being
   compared against stock Flite. Elsewhere — MSVC, musl, a BSD — this makes the
   reference behave as it would have on glibc, so the comparison can run at
   all. Either way both sides are pinned to one documented generator rather
   than to the host, which is the only way the question "do these agree?" has
   an answer.

   refrand.h is force-included ahead of every translation unit, by -include on
   cc and /FI on MSVC, which is also where rand is redirected here and
   RAND_MAX is set to glibc's. See tools/reference/build.py.

   src/clustergen/random.rs is the Rust side of this, and its tests check the
   sequence below against the values rand() is universally known to produce
   from the default seed. */

/* glibc's random() with the default state, i.e. srandom(1).

   TYPE_3: a 31-element additive-feedback sequence, seeded from a Lehmer
   generator, with the first 310 outputs discarded. The low-order bit of the
   sum has a short period, which is why the result is the top 31 bits. */

#define DEGREE 31
#define SEPARATION 3
#define DISCARD (10 * DEGREE)

static unsigned int state[DEGREE];
static int front, rear;
static int seeded = 0;

static unsigned int next_value(void)
{
    unsigned int value;

    /* Unsigned arithmetic, so the addition wraps at 2^32 as glibc's does. */
    state[front] += state[rear];
    value = state[front] >> 1;
    front = (front + 1) % DEGREE;
    rear = (rear + 1) % DEGREE;
    return value;
}

static void seed(void)
{
    int i;

    state[0] = 1;
    for (i = 1; i < DEGREE; i++)
    {
        /* r[i] = 16807 * r[i-1] mod 2^31-1, by Schrage's method so that the
           intermediate product stays inside 32 signed bits. */
        long previous = (long)state[i - 1];
        long hi = previous / 127773;
        long lo = previous % 127773;
        long word = 16807 * lo - 2836 * hi;
        if (word < 0)
            word += 2147483647L;
        state[i] = (unsigned int)word;
    }
    front = SEPARATION;
    rear = 0;
    seeded = 1;
    for (i = 0; i < DISCARD; i++)
        next_value();
}

int flite_ref_rand(void)
{
    if (!seeded)
        seed();
    return (int)next_value();
}

/* srand is left alone: nothing in Flite calls it, and a caller that did would
   be asking for a different sequence than the one being pinned here. */
