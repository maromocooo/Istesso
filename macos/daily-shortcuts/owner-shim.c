// The owner selects the app at build time. Launch arguments cannot change it.
#include <errno.h>
#include <limits.h>
#include <pwd.h>
#include <spawn.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/wait.h>
#include <unistd.h>

int main(void) {
    char *const args[] = {DAILY_SCRIPT, "open", OWNER_APP, "--host", "mac", NULL};
    char tmpdir[sizeof("TMPDIR=") - 1 + PATH_MAX] = "TMPDIR=";
    // The Darwin lookup can fall back to TMPDIR. Only accept the system directory.
    if (unsetenv("TMPDIR") != 0) return 1;
    size_t len = confstr(_CS_DARWIN_USER_TEMP_DIR, tmpdir + sizeof("TMPDIR=") - 1, PATH_MAX);
    struct passwd *account = getpwuid(getuid());
    char *home, *user, *logname, *icon;
    if (!account || !account->pw_dir || !account->pw_dir[0] ||
        !account->pw_name || !account->pw_name[0] || len <= 1 || len > PATH_MAX ||
        asprintf(&icon, "ISTESSO_APP_ICON=%s/Applications/Istesso Dev/%s.app/Contents/Resources/Shortcut.icns",
                 account->pw_dir, SHORTCUT_TITLE) < 0 ||
        asprintf(&home, "HOME=%s", account->pw_dir) < 0 ||
        asprintf(&user, "USER=%s", account->pw_name) < 0 ||
        asprintf(&logname, "LOGNAME=%s", account->pw_name) < 0) {
        fputs("Could not build the owner shortcut environment.\n", stderr);
        return 1;
    }
    char *const env[] = {home, user, logname, icon, "PATH=/usr/bin:/bin:/usr/sbin:/sbin",
                         tmpdir, "LANG=en_US.UTF-8", "DAILY_OWNER=1", "DAILY_OPEN_HANDLER=1", NULL};
    pid_t child;
    if (posix_spawn(&child, DAILY_SCRIPT, NULL, NULL, args, env) != 0) {
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
