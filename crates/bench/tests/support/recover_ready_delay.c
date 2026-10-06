/* Hold only the test's crash child after READY until its parent checks ACK. */
#define _GNU_SOURCE
#include <fcntl.h>
#include <stdlib.h>
#include <string.h>
#include <sys/syscall.h>
#include <unistd.h>

ssize_t write(int fd, const void *bytes, size_t length) {
    int ready = fd == STDOUT_FILENO && length == 6 &&
                memcmp(bytes, "READY\n", 6) == 0;
    if (ready) {
        const char *path = getenv("REDLINE_READY_TEST_HOOK_MARKER");
        if (path) {
            int marker = open(path, O_WRONLY | O_CREAT | O_EXCL, 0600);
            if (marker >= 0) {
                syscall(SYS_write, marker, "READY hook reached\n", 19);
                close(marker);
            }
        }
    }
    ssize_t result = syscall(SYS_write, fd, bytes, length);
    if (ready) {
        const char *release = getenv("REDLINE_READY_TEST_HOOK_RELEASE");
        /* jankurai:allow HLT-008-FALSE-GREEN-RISK reason=libc_nonzero_failure_is_not_a_Jest_skipped_test expires=2027-04-06 */
        if (!release) _exit(97);
        for (int retry = 0; retry < 18000; retry++) {
            if (access(release, F_OK) == 0) return result;
            usleep(10000);
        }
        /* Never resume unverified work after a dead or delayed parent. */
        /* jankurai:allow HLT-008-FALSE-GREEN-RISK reason=libc_nonzero_failure_is_not_a_Jest_skipped_test expires=2027-04-06 */
        _exit(97);
    }
    return result;
}
