/* Force-included ahead of every upstream translation unit, so that rand() is
   the pinned generator in refrand.c rather than the host C library's.

   refrand.c explains why. In short: upstream's Clustergen excitation calls
   rand(), which is unspecified, so without this the reference's output is a
   function of the compiler that built it and "bit-identical to upstream" has
   no meaning for a CG voice.

   stdlib.h is included first, before the macro exists, so that the real
   declaration of rand is never seen through it — redefining a dllimport
   declaration is an error on MSVC. Any later #include <stdlib.h> is a no-op
   because of its own include guard.

   RAND_MAX is replaced too, because plus_or_minus_one() compares against
   RAND_MAX/2.0 and the shim returns glibc's values: with MSVC's RAND_MAX of
   32767 the comparison would be against 16383.5 and almost always true. */

#ifndef FLITE_RS_REFRAND_H
#define FLITE_RS_REFRAND_H

#include <stdlib.h>

int flite_ref_rand(void);

#undef RAND_MAX
#define RAND_MAX 2147483647

#define rand flite_ref_rand

#endif
