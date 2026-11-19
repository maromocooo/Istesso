// The owner selects the app at build time. Launch arguments cannot change it.
#include <errno.h>
#include <spawn.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/wait.h>

extern char **environ;

int main(void) {
    char *const args[] = {DAILY_SCRIPT, "open", OWNER_APP, "--host", "mac", NULL};
    pid_t child;
    if (setenv("DAILY_OWNER", "1", 1) || setenv("DAILY_OPEN_HANDLER", "1", 1)) {
        perror("Could not set the owner shortcut environment");
        return 1;
    }
    if (posix_spawn(&child, DAILY_SCRIPT, NULL, NULL, args, environ) != 0) {
        fputs("Could not start daily.sh in the trial checkout.\n", stderr);
        return 1;
    }
    int status;
    while (waitpid(child, &status, 0) == -1) {
        if (errno == EINTR) continue;
        perror("Could not wait for daily.sh");
        return 1;
    }
    if (WIFEXITED(status)) return WEXITSTATUS(status);
    return WIFSIGNALED(status) ? 128 + WTERMSIG(status) : 1;
}
