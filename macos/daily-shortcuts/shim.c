// Each bundle is built with one literal URL. Finder arguments are ignored.
#include <unistd.h>

#ifndef SHORTCUT_TEST
#define SHORTCUT_OPENER "/usr/bin/open"
#endif

int main(void) {
    execl(SHORTCUT_OPENER, "open", SHORTCUT_URL, (char *)0);
    return 1;
}
