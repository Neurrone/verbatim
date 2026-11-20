/* Force-included into eSpeak NG's MSVC build by build.rs.
 *
 * eSpeak NG's compatibility shims (src/include/compat/unistd.h and
 * endian.h) test `#if __has_include_next(<header>)`. MSVC defines no such
 * macro, and from version 19.52 an undefined function-like macro in `#if`
 * is error C7780 rather than 0. MSVC has neither header, so 0 is the
 * answer the shims need.
 */
#ifndef __has_include_next
#define __has_include_next(header) 0
#endif
