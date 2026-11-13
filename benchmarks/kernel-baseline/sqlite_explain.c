/* Read-only planner probe: never step the underlying SELECT.
 * Usage: sqlite_explain DATABASE asserted|inferred|closure S P O
 * Link to the release engine's exact libsqlite3.a and matching sqlite3.h.
 */
#include <sqlite3.h>
#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static void quoted(const char *text) {
    putchar('"');
    for (const unsigned char *p = (const unsigned char *)text; *p; ++p) {
        if (*p == '"' || *p == '\\') { putchar('\\'); putchar(*p); }
        else if (*p < 32) printf("\\u%04x", *p);
        else putchar(*p);
    }
    putchar('"');
}

static void fail(sqlite3 *db, const char *message) {
    fprintf(stderr, "%s: %s\n", message, db ? sqlite3_errmsg(db) : "invalid argument");
    if (db) sqlite3_close(db);
    exit(1);
}

static sqlite3_int64 pragma_value(sqlite3 *db, const char *sql) {
    sqlite3_stmt *statement = NULL;
    if (sqlite3_prepare_v2(db, sql, -1, &statement, NULL) != SQLITE_OK)
        fail(db, "prepare connection PRAGMA");
    if (sqlite3_step(statement) != SQLITE_ROW) fail(db, "read connection PRAGMA");
    sqlite3_int64 value = sqlite3_column_int64(statement, 0);
    if (sqlite3_step(statement) != SQLITE_DONE) fail(db, "finish connection PRAGMA");
    sqlite3_finalize(statement);
    return value;
}

int main(int argc, char **argv) {
    if (argc != 6) fail(NULL, "usage: sqlite_explain DATABASE asserted|inferred|closure S P O");
    const char *sql;
    if (strcmp(argv[2], "asserted") == 0)
        sql = "SELECT DISTINCT s,p,o,0 FROM facts WHERE explicit=1 AND (s,p,o)>(?,?,?) ORDER BY s,p,o LIMIT 256";
    else if (strcmp(argv[2], "inferred") == 0)
        sql = "SELECT DISTINCT s,p,o,0 FROM facts WHERE explicit=0 AND (s,p,o)>(?,?,?) ORDER BY s,p,o LIMIT 256";
    else if (strcmp(argv[2], "closure") == 0)
        sql = "SELECT DISTINCT s,p,o,0 FROM facts WHERE (s,p,o)>(?,?,?) ORDER BY s,p,o LIMIT 256";
    else fail(NULL, "unknown view");
    sqlite3_int64 bindings[3];
    for (int i = 0; i < 3; ++i) {
        char *end = NULL;
        errno = 0;
        bindings[i] = strtoll(argv[i + 3], &end, 10);
        if (errno || !end || *end || bindings[i] < 0) fail(NULL, "invalid nonnegative cursor");
    }
    sqlite3 *db = NULL;
    if (sqlite3_open_v2(argv[1], &db, SQLITE_OPEN_READONLY | SQLITE_OPEN_NOMUTEX, NULL) != SQLITE_OK)
        fail(db, "open readonly database");
    if (sqlite3_db_readonly(db, "main") != 1) fail(db, "database is not readonly");
    /* These connection-local settings exactly match src/disk.rs. */
    const char *settings = "PRAGMA query_only=ON; PRAGMA mmap_size=0; PRAGMA cache_size=-4096; PRAGMA temp_store=FILE;";
    if (sqlite3_exec(db, settings, NULL, NULL, NULL) != SQLITE_OK) fail(db, "set connection PRAGMAs");
    sqlite3_int64 query_only = pragma_value(db, "PRAGMA query_only");
    sqlite3_int64 mmap_size = pragma_value(db, "PRAGMA mmap_size");
    sqlite3_int64 cache_size = pragma_value(db, "PRAGMA cache_size");
    sqlite3_int64 temp_store = pragma_value(db, "PRAGMA temp_store");
    if (query_only != 1 || mmap_size != 0 || cache_size != -4096 || temp_store != 1)
        fail(db, "connection PRAGMA mismatch");
    char explained[512];
    snprintf(explained, sizeof explained, "EXPLAIN QUERY PLAN %s", sql);
    sqlite3_stmt *statement = NULL;
    if (sqlite3_prepare_v2(db, explained, -1, &statement, NULL) != SQLITE_OK)
        fail(db, "prepare EXPLAIN QUERY PLAN");
    if (sqlite3_stmt_isexplain(statement) != 2) fail(db, "refuse a non-EQP statement");
    for (int i = 0; i < 3; ++i)
        if (sqlite3_bind_int64(statement, i + 1, bindings[i]) != SQLITE_OK)
            fail(db, "bind representative cursor");

    printf("{\"database\":"); quoted(argv[1]);
    printf(",\"view\":"); quoted(argv[2]);
    printf(",\"database_readonly\":true,\"open_flags\":[\"SQLITE_OPEN_READONLY\",\"SQLITE_OPEN_NOMUTEX\"]");
    printf(",\"sqlite_version\":"); quoted(sqlite3_libversion());
    printf(",\"sqlite_version_number\":%d,\"sqlite_source_id\":", sqlite3_libversion_number());
    quoted(sqlite3_sourceid());
    printf(",\"sqlite_compile_options\":[");
    for (int i = 0; sqlite3_compileoption_get(i); ++i) {
        if (i) putchar(',');
        quoted(sqlite3_compileoption_get(i));
    }
    printf("],\"pragmas\":{\"query_only\":%lld,\"mmap_size\":%lld,\"cache_size\":%lld,\"temp_store\":%lld}",
           (long long)query_only, (long long)mmap_size, (long long)cache_size, (long long)temp_store);
    printf(",\"sql\":"); quoted(sql);
    printf(",\"explain_sql\":"); quoted(explained);
    printf(",\"bind_parameters\":[%lld,%lld,%lld],\"statement_is_explain\":2,\"query_was_executed\":false",
           (long long)bindings[0], (long long)bindings[1], (long long)bindings[2]);
    printf(",\"plan_column_names\":[");
    for (int i = 0; i < sqlite3_column_count(statement); ++i) {
        if (i) putchar(',');
        quoted(sqlite3_column_name(statement, i));
    }
    printf("],\"plan_rows\":[");
    int code, first = 1;
    while ((code = sqlite3_step(statement)) == SQLITE_ROW) {
        if (!first) putchar(',');
        first = 0;
        printf("{\"id\":%d,\"parent\":%d,\"aux\":%d,\"detail\":",
               sqlite3_column_int(statement, 0), sqlite3_column_int(statement, 1), sqlite3_column_int(statement, 2));
        quoted((const char *)sqlite3_column_text(statement, 3));
        putchar('}');
    }
    if (code != SQLITE_DONE) fail(db, "step EXPLAIN QUERY PLAN");
    printf("]}\n");
    sqlite3_finalize(statement);
    if (sqlite3_close(db) != SQLITE_OK) fail(NULL, "close readonly database");
    return 0;
}
