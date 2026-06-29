/*
 * fchmod_compat.c — Fixes Zig 0.13's fchmod on O_PATH file descriptors.
 *
 * Zig 0.13 opens directories without `OpenDirOptions{ .iterate = true }` using
 * O_PATH, which yields a file descriptor that the kernel rejects for fchmod(2)
 * with EBADF. Zig's stdlib treats EBADF from fchmod as unreachable (panic).
 *
 * This shim intercepts fchmod and, when the syscall returns EBADF, retries via
 * fchmodat(AT_FDCWD, "/proc/self/fd/<n>", mode, 0) which allows chmod through
 * an O_PATH descriptor. All other errors pass through unchanged.
 *
 * Linked before libc in the test binary so this definition wins.
 */
#define _GNU_SOURCE
#include <fcntl.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <unistd.h>
#include <stdio.h>
#include <errno.h>

int fchmod(int fd, mode_t mode)
{
    long r = syscall(__NR_fchmod, (long)fd, (long)mode);
    if (r == 0) return 0;

    /* If EBADF, the fd may be an O_PATH fd (opened without iterate=true by
     * Zig 0.13 Dir.makeOpenPath with default options). Retry via /proc/self/fd
     * which allows chmod through an O_PATH descriptor. */
    if (errno == EBADF) {
        char path[64];
        snprintf(path, sizeof(path), "/proc/self/fd/%d", fd);
        r = syscall(__NR_fchmodat, (long)AT_FDCWD, (long)path, (long)mode, 0L);
        if (r == 0) return 0;
    }

    return -1;
}
