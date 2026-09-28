/* Independent C ABI probe for the SQLite-compatible surface.
 *
 * Every declaration comes from the vendored upstream SQLite 3.53.1 sqlite3.h
 * (contracts/c-abi/upstream/sqlite-3.53.1), never from RedlineDB's headers,
 * so a signature or constant that differs from upstream shows up as a wrong
 * result or a crash instead of the same mistake on both sides.
 *
 * Dynamic build: argv[1] is a shared library; every entry point is resolved
 * with dlsym and must come from that file. Static build (-DPROBE_STATIC): the
 * entry points are linked from a static archive and argv[1] is only a label.
 *
 * Usage: probe <library-or-label> <case> <database-path>
 * Exit status: 0 pass, 1 failed check, 125 harness problem.
 */
#define _GNU_SOURCE
#include <sqlite3.h>
#ifndef PROBE_STATIC
#include <dlfcn.h>
#endif
#include <limits.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

#define API_LIST(X)                                                                  \
    X(sqlite3_open) X(sqlite3_close) X(sqlite3_prepare_v2) X(sqlite3_prepare_v3)     \
    X(sqlite3_step) X(sqlite3_finalize) X(sqlite3_column_type) X(sqlite3_column_int64) \
    X(sqlite3_column_text) X(sqlite3_column_bytes) X(sqlite3_column_value)           \
    X(sqlite3_value_type) X(sqlite3_db_filename) X(sqlite3_libversion)               \
    X(sqlite3_sourceid)
#define DECLARE(name) static __typeof__(name) *p_##name;
API_LIST(DECLARE)

#ifdef PROBE_STATIC
/* RedlineDB-only native entry point, used by the native-tags case. */
int rldb_column_type(sqlite3_stmt *stmt, int index);
#else
static void *library;
#endif

#define CHECK(test)                                                    \
    do {                                                               \
        if (!(test)) {                                                 \
            fprintf(stderr, "FAIL line %d: %s\n", __LINE__, #test);    \
            return 1;                                                  \
        }                                                              \
    } while (0)

#define SENTINEL ((sqlite3_stmt *)(uintptr_t)1)

static int load(const char *wanted) {
#ifdef PROBE_STATIC
    (void)wanted;
#define BIND(name) p_##name = name;
    API_LIST(BIND)
    return 0;
#else
    library = dlopen(wanted, RTLD_NOW | RTLD_LOCAL);
    if (!library) {
        fprintf(stderr, "%s\n", dlerror());
        return 125;
    }
#define LOAD(name)                                                                  \
    do {                                                                            \
        void *symbol = dlsym(library, #name);                                       \
        Dl_info info;                                                               \
        char actual[PATH_MAX];                                                      \
        if (!symbol || !dladdr(symbol, &info) || !realpath(info.dli_fname, actual) || \
            strcmp(actual, wanted)) {                                               \
            fprintf(stderr, "wrong or missing symbol: %s\n", #name);                \
            return 125;                                                             \
        }                                                                           \
        memcpy(&p_##name, &symbol, sizeof(symbol));                                 \
    } while (0);
    API_LIST(LOAD)
    return 0;
#endif
}

/* Two pages; the second is PROT_NONE. Returns the first byte of the guard. */
static char *guard_edge(char **mapping, size_t *length) {
    long page = sysconf(_SC_PAGESIZE);
    if (page <= 0) return NULL;
    *length = (size_t)page * 2;
    *mapping = mmap(NULL, *length, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (*mapping == MAP_FAILED) return NULL;
    if (mprotect(*mapping + page, (size_t)page, PROT_NONE) != 0) return NULL;
    return *mapping + page;
}

/* Prepare and step one statement; returns the step result (or -1). */
static int run_one(sqlite3 *db, const char *sql, sqlite3_int64 *first) {
    sqlite3_stmt *stmt = NULL;
    if (p_sqlite3_prepare_v2(db, sql, -1, &stmt, NULL) != SQLITE_OK || !stmt) return -1;
    int rc = p_sqlite3_step(stmt);
    if (rc == SQLITE_ROW && first) *first = p_sqlite3_column_int64(stmt, 0);
    if (p_sqlite3_finalize(stmt) != SQLITE_OK) return -1;
    return rc;
}

static int run_case(const char *test, sqlite3 *db, const char *db_path) {
    sqlite3_stmt *stmt = SENTINEL;
    const char *tail = NULL;
    const char *sql = "SELECT 7; SELECT 9";
    int rc;
    if (!strcmp(test, "v3-zero") || !strcmp(test, "v3-persistent") || !strcmp(test, "v3-normalize")) {
        unsigned flags = !strcmp(test, "v3-zero")        ? 0
                         : !strcmp(test, "v3-normalize") ? SQLITE_PREPARE_NORMALIZE
                                                         : SQLITE_PREPARE_PERSISTENT;
        rc = p_sqlite3_prepare_v3(db, sql, -1, flags, &stmt, &tail);
        printf("rc=%d stmt_null=%d tail_matches=%d\n", rc, stmt == NULL, tail == sql + 9);
        CHECK(rc == SQLITE_OK && stmt && stmt != SENTINEL && tail == sql + 9);
        CHECK(p_sqlite3_step(stmt) == SQLITE_ROW && p_sqlite3_column_int64(stmt, 0) == 7);
    } else if (!strcmp(test, "bounded-guard") || !strcmp(test, "zero-guard")) {
        char *mapping;
        size_t length;
        char *edge = guard_edge(&mapping, &length);
        CHECK(edge != NULL);
        int bound = !strcmp(test, "zero-guard") ? 0 : 8;
        char *input = edge - bound;
        if (bound) memcpy(input, "SELECT 7", 8); /* deliberately no terminator */
        rc = p_sqlite3_prepare_v2(db, input, bound, &stmt, &tail);
        printf("rc=%d stmt_null=%d tail_matches=%d\n", rc, stmt == NULL, tail == input + bound);
        CHECK(rc == SQLITE_OK && tail == input + bound);
        if (!bound) {
            CHECK(stmt == NULL);
        } else {
            CHECK(stmt && stmt != SENTINEL);
            CHECK(p_sqlite3_step(stmt) == SQLITE_ROW && p_sqlite3_column_int64(stmt, 0) == 7);
            CHECK(p_sqlite3_finalize(stmt) == SQLITE_OK);
            stmt = NULL;
        }
        CHECK(munmap(mapping, length) == 0);
    } else if (!strcmp(test, "early-nul-guard") || !strcmp(test, "negative-guard")) {
        char *mapping;
        size_t length;
        char *edge = guard_edge(&mapping, &length);
        CHECK(edge != NULL);
        char *input = edge - 9;
        memcpy(input, "SELECT 7", 9); /* the NUL is the last readable byte */
        int bound = !strcmp(test, "negative-guard") ? -7 : INT_MAX;
        CHECK(p_sqlite3_prepare_v3(db, input, bound, 0, &stmt, &tail) == SQLITE_OK);
        CHECK(stmt && stmt != SENTINEL && tail == input + 8);
        CHECK(p_sqlite3_step(stmt) == SQLITE_ROW && p_sqlite3_column_int64(stmt, 0) == 7);
        CHECK(p_sqlite3_finalize(stmt) == SQLITE_OK);
        stmt = NULL;
        CHECK(munmap(mapping, length) == 0);
    } else if (!strcmp(test, "repeat-tail")) {
        for (int i = 0; i < 32; ++i) {
            const char *input = "SELECT 7; SELECT 9; -- done";
            CHECK(p_sqlite3_prepare_v3(db, input, (int)strlen(input), 3, &stmt, &tail) == SQLITE_OK);
            CHECK(stmt && tail == input + 9);
            CHECK(p_sqlite3_step(stmt) == SQLITE_ROW && p_sqlite3_column_int64(stmt, 0) == 7);
            CHECK(p_sqlite3_finalize(stmt) == SQLITE_OK);
            CHECK(p_sqlite3_prepare_v3(db, tail, -1, 0, &stmt, &tail) == SQLITE_OK);
            CHECK(stmt && p_sqlite3_step(stmt) == SQLITE_ROW && p_sqlite3_column_int64(stmt, 0) == 9);
            CHECK(p_sqlite3_finalize(stmt) == SQLITE_OK);
            CHECK(p_sqlite3_prepare_v3(db, tail, -1, 0, &stmt, &tail) == SQLITE_OK);
            CHECK(stmt == NULL && tail == input + strlen(input));
        }
    } else if (!strcmp(test, "unsupported-flags")) {
        /* RedlineDB refuses flags whose semantics it does not implement;
         * upstream accepts them, so this case is RedlineDB-only. */
        unsigned flags[] = {SQLITE_PREPARE_NO_VTAB, SQLITE_PREPARE_DONT_LOG, SQLITE_PREPARE_FROM_DDL,
                            0x80u, 0x80000000u};
        for (unsigned i = 0; i < sizeof(flags) / sizeof(flags[0]); ++i) {
            stmt = SENTINEL;
            CHECK(p_sqlite3_prepare_v3(db, sql, -1, flags[i], &stmt, &tail) == SQLITE_ERROR);
            CHECK(stmt == NULL);
        }
    } else if (!strcmp(test, "native-tags")) {
        int (*native_type)(sqlite3_stmt *, int) = NULL;
#ifdef PROBE_STATIC
        native_type = rldb_column_type;
#else
        void *symbol = dlsym(library, "rldb_column_type");
        CHECK(symbol != NULL);
        memcpy(&native_type, &symbol, sizeof(symbol));
#endif
        CHECK(p_sqlite3_prepare_v2(db, "SELECT NULL, 7, 1.5, 'abc', x'0001'", -1, &stmt, NULL) == SQLITE_OK);
        CHECK(p_sqlite3_step(stmt) == SQLITE_ROW);
        for (int i = 0; i < 5; ++i) CHECK(native_type(stmt, i) == i); /* RLDB_NULL is 0 */
    } else if (!strcmp(test, "empty-tail")) {
        sql = "  -- comment\n /* empty */ ";
        rc = p_sqlite3_prepare_v2(db, sql, -1, &stmt, &tail);
        printf("rc=%d stmt_null=%d tail_matches=%d\n", rc, stmt == NULL, tail == sql + strlen(sql));
        CHECK(rc == SQLITE_OK && stmt == NULL && tail == sql + strlen(sql));
    } else if (!strcmp(test, "embedded-nul")) {
        static const char input[] = "SELECT 7\0SELECT 9";
        rc = p_sqlite3_prepare_v2(db, input, sizeof(input) - 1, &stmt, &tail);
        printf("rc=%d tail_matches=%d\n", rc, tail == input + 8);
        CHECK(rc == SQLITE_OK && stmt && stmt != SENTINEL && tail == input + 8);
        CHECK(p_sqlite3_step(stmt) == SQLITE_ROW && p_sqlite3_column_int64(stmt, 0) == 7);
    } else if (!strcmp(test, "error-output") || !strcmp(test, "v3-error-output")) {
        rc = !strcmp(test, "error-output") ? p_sqlite3_prepare_v2(db, "SELECT FROM", -1, &stmt, &tail)
                                           : p_sqlite3_prepare_v3(db, "SELECT FROM", -1, 3, &stmt, &tail);
        printf("rc=%d stmt_null=%d\n", rc, stmt == NULL);
        CHECK(rc == SQLITE_ERROR && stmt == NULL);
    } else if (!strcmp(test, "v2-tail")) {
        rc = p_sqlite3_prepare_v2(db, sql, -1, &stmt, &tail);
        printf("rc=%d tail_matches=%d\n", rc, tail == sql + 9);
        CHECK(rc == SQLITE_OK && stmt && stmt != SENTINEL && tail == sql + 9);
        CHECK(p_sqlite3_step(stmt) == SQLITE_ROW && p_sqlite3_column_int64(stmt, 0) == 7);
    } else if (!strcmp(test, "type-tags")) {
        int expected[] = {SQLITE_NULL, SQLITE_INTEGER, SQLITE_FLOAT, SQLITE_TEXT, SQLITE_BLOB};
        CHECK(p_sqlite3_prepare_v2(db, "SELECT NULL, 7, 1.5, 'abc', x'0001'", -1, &stmt, NULL) == SQLITE_OK);
        CHECK(p_sqlite3_step(stmt) == SQLITE_ROW);
        int mismatch = 0;
        for (int i = 0; i < 5; ++i) {
            int actual = p_sqlite3_column_type(stmt, i);
            printf("column=%d actual=%d expected=%d\n", i, actual, expected[i]);
            mismatch |= actual != expected[i];
            CHECK(p_sqlite3_value_type(p_sqlite3_column_value(stmt, i)) == expected[i]);
        }
        CHECK(!mismatch);
    } else if (!strcmp(test, "text-conversions")) {
        static const struct {
            int type;
            const char *text;
            int bytes;
        } want[] = {
            {SQLITE_INTEGER, "7", 1}, {SQLITE_FLOAT, "1.5", 3}, {SQLITE_NULL, NULL, 0},
            {SQLITE_TEXT, "x", 1},    {SQLITE_BLOB, "a\0b", 3}, {SQLITE_TEXT, "a\0b", 3},
            {SQLITE_FLOAT, "1.0", 3},
        };
        CHECK(p_sqlite3_prepare_v2(db, "SELECT 7, 1.5, NULL, 'x', x'610062', char(97, 0, 98), 1.0", -1,
                                   &stmt, NULL) == SQLITE_OK);
        CHECK(p_sqlite3_step(stmt) == SQLITE_ROW);
        for (int i = 0; i < (int)(sizeof(want) / sizeof(want[0])); ++i) {
            /* Read the class first: upstream may change it after a conversion. */
            int type = p_sqlite3_column_type(stmt, i);
            const unsigned char *text = p_sqlite3_column_text(stmt, i);
            int bytes = p_sqlite3_column_bytes(stmt, i);
            printf("column=%d type=%d bytes=%d text_null=%d\n", i, type, bytes, text == NULL);
            CHECK(type == want[i].type);
            CHECK(bytes == want[i].bytes);
            if (!want[i].text) {
                CHECK(text == NULL);
            } else {
                CHECK(text && !memcmp(text, want[i].text, (size_t)bytes) && text[bytes] == 0);
            }
        }
    } else if (!strcmp(test, "memory-open")) {
        /* Work in the database's directory so a stray ":memory:" file is
         * visible, then prove the in-memory database works and left none. */
        char dir[PATH_MAX];
        CHECK(strlen(db_path) < sizeof(dir));
        strcpy(dir, db_path);
        char *slash = strrchr(dir, '/');
        CHECK(slash != NULL);
        *slash = 0;
        CHECK(chdir(dir) == 0);
        sqlite3 *memory = NULL;
        CHECK(p_sqlite3_open(":memory:", &memory) == SQLITE_OK && memory);
        const char *name = p_sqlite3_db_filename(memory, "main");
        CHECK(name != NULL && name[0] == 0);
        sqlite3_int64 value = 0;
        CHECK(run_one(memory, "CREATE TABLE t(x)", NULL) == SQLITE_DONE);
        CHECK(run_one(memory, "INSERT INTO t VALUES (42)", NULL) == SQLITE_DONE);
        CHECK(run_one(memory, "SELECT x FROM t", &value) == SQLITE_ROW && value == 42);
        CHECK(p_sqlite3_close(memory) == SQLITE_OK);
        CHECK(access(":memory:", F_OK) != 0);
        stmt = NULL;
    } else {
        return 125;
    }
    if (stmt && stmt != SENTINEL) CHECK(p_sqlite3_finalize(stmt) == SQLITE_OK);
    return 0;
}

int main(int argc, char **argv) {
    if (argc != 4) return 125;
    setbuf(stdout, NULL);
#ifdef PROBE_STATIC
    const char *wanted = argv[1];
#else
    char wanted[PATH_MAX];
    if (!realpath(argv[1], wanted)) return 125;
#endif
    int loaded = load(wanted);
    if (loaded) return loaded;
    printf("loaded=%s\nversion=%s\nsourceid=%s\ncase=%s\n", wanted, p_sqlite3_libversion(),
           p_sqlite3_sourceid(), argv[2]);
    sqlite3 *db = NULL;
    if (p_sqlite3_open(argv[3], &db) != SQLITE_OK) {
        fprintf(stderr, "cannot open %s\n", argv[3]);
        return 125;
    }
    int result = run_case(argv[2], db, argv[3]);
    if (result) return result;
    CHECK(p_sqlite3_close(db) == SQLITE_OK);
#ifndef PROBE_STATIC
    dlclose(library);
#endif
    return 0;
}
