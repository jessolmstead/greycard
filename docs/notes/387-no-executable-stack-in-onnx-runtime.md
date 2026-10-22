# No executable stack in ONNX Runtime (2026-10-21)

Issue 32: on CachyOS the AI features were off with "cannot enable
executable stack as shared object requires". The 0.8.0 Linux
package's `libonnxruntime.so` asked for an executable stack
(`GNU_STACK` RWE); its `libwebgpu_dawn.so` did not. glibc 2.41 and
later refuse to dlopen such a library. One object in pyke's archive
carries no stack note, and the release runner's ld marks the result
executable for it; binutils 2.47 here does not, so a local build
loaded and the bug was not seen.

ort-dylib now links with `-z noexecstack`, and the release checks
every library it ships on Linux for RWE. `GLIBC_TUNABLES=glibc.rtld.execstack=2`
lets an 0.8.0 package load until then.
