/* Forwards rustc and, for the Kova Screen library tests only, links the
 * common-controls manifest. The application binary already has one. */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <windows.h>

static void append(char **buf, size_t *len, size_t *cap, const char *text) {
    size_t add = strlen(text);
    while (*len + add + 1 > *cap) {
        *cap = *cap ? *cap * 2 : 1024;
        char *next = realloc(*buf, *cap);
        if (!next) {
            abort();
        }
        *buf = next;
    }
    memcpy(*buf + *len, text, add + 1);
    *len += add;
}

static void append_arg(char **buf, size_t *len, size_t *cap, const char *arg) {
    int plain = 1;
    if (*arg == '\0') {
        plain = 0;
    }
    for (const char *p = arg; *p; ++p) {
        if (*p == ' ' || *p == '\t' || *p == '"' || *p == ',' || *p == '(' || *p == ')') {
            plain = 0;
            break;
        }
    }
    if (*len > 0) {
        append(buf, len, cap, " ");
    }
    if (plain) {
        append(buf, len, cap, arg);
        return;
    }
    append(buf, len, cap, "\"");
    for (const char *p = arg; *p; ++p) {
        if (*p == '"') {
            append(buf, len, cap, "\\\"");
        } else {
            char one[2] = {*p, '\0'};
            append(buf, len, cap, one);
        }
    }
    append(buf, len, cap, "\"");
}

int main(int argc, char **argv) {
    if (argc < 2) {
        return 1;
    }

    int is_test = 0;
    int is_lib = 0;
    for (int i = 2; i < argc; ++i) {
        if (strcmp(argv[i], "--test") == 0) {
            is_test = 1;
        }
        if (strcmp(argv[i], "--crate-name") == 0 && i + 1 < argc &&
            strcmp(argv[i + 1], "kova_screen_lib") == 0) {
            is_lib = 1;
        }
    }

    char link[MAX_PATH + 16];
    link[0] = '\0';
    if (is_test && is_lib) {
        char exe[MAX_PATH];
        DWORD n = GetModuleFileNameA(NULL, exe, MAX_PATH);
        if (n > 0 && n < MAX_PATH) {
            char *slash = strrchr(exe, '\\');
            if (slash) {
                *slash = '\0';
                char res[MAX_PATH];
                snprintf(res, sizeof(res), "%s\\..\\target\\kova-screen-tests.res", exe);
                if (GetFileAttributesA(res) != INVALID_FILE_ATTRIBUTES) {
                    snprintf(link, sizeof(link), "link-arg=%s", res);
                }
            }
        }
    }

    char *cmd = NULL;
    size_t len = 0;
    size_t cap = 0;
    for (int i = 1; i < argc; ++i) {
        append_arg(&cmd, &len, &cap, argv[i]);
    }
    if (link[0]) {
        append_arg(&cmd, &len, &cap, "-C");
        append_arg(&cmd, &len, &cap, link);
    }

    STARTUPINFOA si;
    PROCESS_INFORMATION pi;
    memset(&si, 0, sizeof(si));
    si.cb = sizeof(si);
    memset(&pi, 0, sizeof(pi));
    BOOL ok = CreateProcessA(NULL, cmd, NULL, NULL, TRUE, 0, NULL, NULL, &si, &pi);
    free(cmd);
    if (!ok) {
        fprintf(stderr, "rustc wrapper could not start the compiler\n");
        return 1;
    }
    WaitForSingleObject(pi.hProcess, INFINITE);
    DWORD code = 1;
    GetExitCodeProcess(pi.hProcess, &code);
    CloseHandle(pi.hThread);
    CloseHandle(pi.hProcess);
    return (int)code;
}
