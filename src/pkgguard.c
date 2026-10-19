// Preloaded into the PS4 PKG extractor (see src/pkgx.rs).
//
// The extractor writes with stdio and ignores every result, so a full disk, a quota or an I/O
// error would leave truncated files behind a normal "success" report. This stops it instead:
// a failed open-for-writing, write, flush, truncate or close ends the process with a message
// the launcher shows. When a file is closed, its size on disk must also cover every byte written
// to it. Nothing else is changed.

#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <pthread.h>
#include <stdio.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <unistd.h>

#define TRACKED 256

static struct { FILE *file; off_t end; } files[TRACKED];
static pthread_mutex_t lock = PTHREAD_MUTEX_INITIALIZER;

// The launcher reads stdout and reports lines starting with "Cannot".
static void fail(const char *what, int err) {
    char line[256];
    int n = snprintf(line, sizeof line, "\nCannot write the extracted files (%s): %s\n", what, strerror(err));
    if (n > 0 && write(STDOUT_FILENO, line, (size_t)n) < 0) { /* nothing more to do */ }
    _exit(86);
}

static void *real(const char *name) {
    void *f = dlsym(RTLD_NEXT, name);
    if (!f) fail(name, ENOSYS);
    return f;
}

// Only the extractor itself is checked, not the tools its AppImage start-up script runs (their
// ordinary broken-pipe writes, e.g. `ls | head`, aren't failures).
static int active(void) {
    static int on = -1;
    if (on < 0) {
        char exe[4096];
        ssize_t n = readlink("/proc/self/exe", exe, sizeof exe - 1);
        exe[n > 0 ? n : 0] = 0;
        const char *base = strrchr(exe, '/');
        on = base && strcmp(base + 1, "pkg_extractor") == 0;
    }
    return on;
}

// The launcher only accepts an extraction whose output carries this line, so a guard that failed
// to load (the dynamic loader just warns and carries on) can't go unnoticed.
__attribute__((constructor)) static void announce(void) {
    static const char line[] = "PKG write guard active\n";
    if (active() && write(STDOUT_FILENO, line, sizeof line - 1) < 0) _exit(86);
}

static int writing(const char *mode) {
    return mode && (strchr(mode, 'w') || strchr(mode, 'a') || strchr(mode, '+'));
}

static void track(FILE *f, off_t end) {
    pthread_mutex_lock(&lock);
    int free_slot = -1;
    for (int i = 0; i < TRACKED; i++) {
        if (files[i].file == f) { if (end > files[i].end) files[i].end = end; free_slot = -2; break; }
        if (!files[i].file && free_slot == -1) free_slot = i;
    }
    if (free_slot >= 0) { files[free_slot].file = f; files[free_slot].end = end; }
    pthread_mutex_unlock(&lock);
}

static off_t forget(FILE *f) {
    off_t end = -1;
    pthread_mutex_lock(&lock);
    for (int i = 0; i < TRACKED; i++)
        if (files[i].file == f) { end = files[i].end; files[i].file = NULL; break; }
    pthread_mutex_unlock(&lock);
    return end;
}

static FILE *opened(FILE *f, const char *mode) {
    if (!active()) return f;
    if (!f && writing(mode)) fail("open", errno);
    if (f && writing(mode)) track(f, 0);
    return f;
}

FILE *fopen(const char *path, const char *mode) {
    static FILE *(*next)(const char *, const char *);
    if (!next) next = real("fopen");
    return opened(next(path, mode), mode);
}

FILE *fopen64(const char *path, const char *mode) {
    static FILE *(*next)(const char *, const char *);
    if (!next) next = real("fopen64");
    return opened(next(path, mode), mode);
}

size_t fwrite(const void *data, size_t size, size_t count, FILE *f) {
    static size_t (*next)(const void *, size_t, size_t, FILE *);
    if (!next) next = real("fwrite");
    size_t done = next(data, size, count, f);
    if (!active()) return done;
    if (size && done != count) fail("write", errno ? errno : EIO);
    if (f != stdout && f != stderr && size && count) {
        off_t at = ftello(f);
        if (at >= 0) track(f, at);
    }
    return done;
}

int fflush(FILE *f) {
    static int (*next)(FILE *);
    if (!next) next = real("fflush");
    int r = next(f);
    if (r != 0 && active()) fail("flush", errno);
    return r;
}

int ftruncate(int fd, off_t length) {
    static int (*next)(int, off_t);
    if (!next) next = real("ftruncate");
    int r = next(fd, length);
    if (!active()) return r;
    if (r != 0) fail("truncate", errno);
    // The file is now exactly `length` long; later writes extend it again.
    pthread_mutex_lock(&lock);
    for (int i = 0; i < TRACKED; i++)
        if (files[i].file && fileno(files[i].file) == fd) files[i].end = length;
    pthread_mutex_unlock(&lock);
    return r;
}

// glibc declares fclose's FILE as never NULL, and newer GCCs then reject the check below; but this
// stands in for fclose in other programs, which can pass NULL, so the check stays.
#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Wnonnull-compare"
int fclose(FILE *f) {
    static int (*next)(FILE *);
    if (!next) next = real("fclose");
    off_t end = f && active() ? forget(f) : -1;
    if (end >= 0) {
        // Everything written must have reached the file: flush, then compare its real size.
        static int (*flush)(FILE *);
        if (!flush) flush = real("fflush");
        if (flush(f) != 0 || ferror(f)) fail("flush", errno ? errno : EIO);
        struct stat st;
        if (fstat(fileno(f), &st) != 0) fail("size check", errno);
        if (st.st_size < end) fail("size check: file shorter than written", EIO);
    }
    int r = next(f);
    if (r != 0 && end >= 0) fail("close", errno);
    return r;
}
#pragma GCC diagnostic pop
