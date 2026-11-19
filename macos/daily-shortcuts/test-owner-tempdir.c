// Provide a deterministic temp directory when the sandbox blocks the Darwin lookup.
#include <string.h>
#include <unistd.h>

size_t fixture_confstr(int name, char *buffer, size_t size) {
    const char path[] = "/fixture/tmp/";
    if (name != _CS_DARWIN_USER_TEMP_DIR) return 0;
    if (buffer && size) strlcpy(buffer, path, size);
    return sizeof(path);
}
