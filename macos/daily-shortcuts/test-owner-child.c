// Stand-in for daily.sh. Check the spawn environment before a shell can change it.
#include <limits.h>
#include <pwd.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

extern char **environ;

int main(int argc, char **argv) {
    // Determine the expected values independently of the environment being tested.
    char **received = environ;
    char *empty[] = {NULL};
    environ = empty;
    char tmpdir[PATH_MAX];
    size_t length = confstr(_CS_DARWIN_USER_TEMP_DIR, tmpdir, sizeof(tmpdir));
    struct passwd *account = getpwuid(getuid());
    environ = received;
    if (!account || !account->pw_dir || !account->pw_dir[0] ||
        !account->pw_name || !account->pw_name[0] || length <= 1 || length > sizeof(tmpdir)) return 1;
    if (argc == 2 && strcmp(argv[1], "--check-sources") == 0) return 0;
    char icon[PATH_MAX];
    snprintf(icon, sizeof(icon), "%s/Applications/Istesso Dev/Fixture.app/Contents/Resources/Shortcut.icns",
             account->pw_dir);
    const char *expected[][2] = {
        {"ISTESSO_APP_ICON", icon},
        {"HOME", account->pw_dir}, {"USER", account->pw_name}, {"LOGNAME", account->pw_name},
        {"PATH", "/usr/bin:/bin:/usr/sbin:/sbin"}, {"TMPDIR", tmpdir}, {"LANG", "en_US.UTF-8"},
        {"DAILY_OWNER", "1"}, {"DAILY_OPEN_HANDLER", "1"},
    };
    size_t count = 0;
    while (environ[count]) count++;
    if (count != sizeof(expected) / sizeof(expected[0])) {
        fputs("Unexpected number of environment entries.\n", stderr);
        return 1;
    }
    for (size_t i = 0; i < count; i++) {
        const char *value = getenv(expected[i][0]);
        if (!value || strcmp(value, expected[i][1]) != 0) {
            fprintf(stderr, "Unexpected value for %s.\n", expected[i][0]);
            return 1;
        }
    }
    printf("%d\n", getppid());
    for (int i = 1; i < argc; i++) printf("%s\n", argv[i]);
    return 37;
}
