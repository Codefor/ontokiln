/* Post-timing direct-read audit. Never link the host's unrelated SQLite.
 * Usage: sqlite_audit DATABASE groups
 *        sqlite_audit DATABASE samples SUBJECT_NTRIPLES ...
 * Link to the release engine's exact libsqlite3.a and matching sqlite3.h.
 */
#include <sqlite3.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static void quoted_bytes(const unsigned char *text, int length) {
    putchar('"');
    for (int i = 0; i < length; ++i) {
        unsigned char c = text[i];
        if (c == '"' || c == '\\') { putchar('\\'); putchar(c); }
        else if (c < 32) printf("\\u%04x", c);
        else putchar(c);
    }
    putchar('"');
}

static void quoted(const char *text) {
    quoted_bytes((const unsigned char *)text, (int)strlen(text));
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

static void column_value(sqlite3_stmt *statement, int column) {
    switch (sqlite3_column_type(statement, column)) {
        case SQLITE_NULL: printf("null"); break;
        case SQLITE_INTEGER:
            printf("%lld", (long long)sqlite3_column_int64(statement, column)); break;
        case SQLITE_TEXT:
            quoted_bytes(sqlite3_column_text(statement, column), sqlite3_column_bytes(statement, column)); break;
        default:
            fprintf(stderr, "unexpected audit column type\n"); exit(1);
    }
}

static void query(sqlite3 *db, const char *name, const char *sql, int parameters, char **values) {
    sqlite3_stmt *statement = NULL;
    if (sqlite3_prepare_v2(db, sql, -1, &statement, NULL) != SQLITE_OK)
        fail(db, "prepare audit SELECT");
    if (!sqlite3_stmt_readonly(statement)) fail(db, "refuse non-readonly audit statement");
    if (sqlite3_bind_parameter_count(statement) != parameters) fail(db, "audit binding count mismatch");
    for (int i = 0; i < parameters; ++i)
        if (sqlite3_bind_text(statement, i + 1, values[i], -1, SQLITE_TRANSIENT) != SQLITE_OK)
            fail(db, "bind audit subject");

    printf("{\"name\":"); quoted(name);
    printf(",\"sql\":"); quoted(sql);
    printf(",\"statement_readonly\":true,\"parameters\":[");
    for (int i = 0; i < parameters; ++i) {
        if (i) putchar(',');
        quoted(values[i]);
    }
    printf("],\"columns\":[");
    int columns = sqlite3_column_count(statement);
    for (int i = 0; i < columns; ++i) {
        if (i) putchar(',');
        quoted(sqlite3_column_name(statement, i));
    }
    printf("],\"rows\":[");
    int code, first = 1;
    while ((code = sqlite3_step(statement)) == SQLITE_ROW) {
        if (!first) putchar(',');
        first = 0;
        putchar('[');
        for (int i = 0; i < columns; ++i) {
            if (i) putchar(',');
            column_value(statement, i);
        }
        putchar(']');
    }
    if (code != SQLITE_DONE) fail(db, "step audit SELECT");
    printf("]}");
    if (sqlite3_finalize(statement) != SQLITE_OK) fail(db, "finalize audit SELECT");
}

int main(int argc, char **argv) {
    if (argc < 3) fail(NULL, "usage: sqlite_audit DATABASE groups|samples [SUBJECT_NTRIPLES ...]");
    int groups = strcmp(argv[2], "groups") == 0;
    int samples = strcmp(argv[2], "samples") == 0;
    if ((!groups && !samples) || (groups && argc != 3) || (samples && (argc < 4 || argc > 67)))
        fail(NULL, "invalid audit mode or subject count");
    sqlite3 *db = NULL;
    if (sqlite3_open_v2(argv[1], &db, SQLITE_OPEN_READONLY | SQLITE_OPEN_NOMUTEX, NULL) != SQLITE_OK)
        fail(db, "open readonly database");
    if (sqlite3_db_readonly(db, "main") != 1) fail(db, "database is not readonly");
    sqlite3_busy_timeout(db, 5000);
    const char *settings = "PRAGMA query_only=ON; PRAGMA mmap_size=0; PRAGMA cache_size=-4096; PRAGMA temp_store=FILE; BEGIN;";
    if (sqlite3_exec(db, settings, NULL, NULL, NULL) != SQLITE_OK)
        fail(db, "set connection PRAGMAs and read transaction");
    sqlite3_int64 query_only = pragma_value(db, "PRAGMA query_only");
    sqlite3_int64 mmap_size = pragma_value(db, "PRAGMA mmap_size");
    sqlite3_int64 cache_size = pragma_value(db, "PRAGMA cache_size");
    sqlite3_int64 temp_store = pragma_value(db, "PRAGMA temp_store");
    if (query_only != 1 || mmap_size != 0 || cache_size != -4096 || temp_store != 1)
        fail(db, "connection PRAGMA mismatch");

    printf("{\"schema_version\":\"ontokiln-post-timing-sqlite-probe/v1\",\"database\":"); quoted(argv[1]);
    printf(",\"mode\":"); quoted(argv[2]);
    printf(",\"database_readonly\":true,\"read_transaction\":true,\"open_flags\":[\"SQLITE_OPEN_READONLY\",\"SQLITE_OPEN_NOMUTEX\"]");
    printf(",\"sqlite_version\":"); quoted(sqlite3_libversion());
    printf(",\"sqlite_version_number\":%d,\"sqlite_source_id\":", sqlite3_libversion_number());
    quoted(sqlite3_sourceid());
    printf(",\"sqlite_compile_options\":[");
    for (int i = 0; sqlite3_compileoption_get(i); ++i) {
        if (i) putchar(',');
        quoted(sqlite3_compileoption_get(i));
    }
    printf("],\"pragmas\":{\"query_only\":%lld,\"mmap_size\":%lld,\"cache_size\":%lld,\"temp_store\":%lld},\"queries\":[",
           (long long)query_only, (long long)mmap_size, (long long)cache_size, (long long)temp_store);
    if (groups) {
        query(db, "fact_totals",
              "SELECT COUNT(*) AS total_facts, "
              "COALESCE(SUM(CASE WHEN explicit=1 THEN 1 ELSE 0 END),0) AS explicit_facts, "
              "COALESCE(SUM(CASE WHEN explicit=0 THEN 1 ELSE 0 END),0) AS inferred_facts, "
              "COALESCE(SUM(CASE WHEN explicit IN (0,1) THEN 0 ELSE 1 END),0) AS invalid_explicit_flags, "
              "COALESCE(SUM(CASE WHEN g=0 THEN 0 ELSE 1 END),0) AS nondefault_graph_facts FROM facts",
              0, NULL);
        putchar(',');
        query(db, "predicate_groups",
              "WITH grouped AS MATERIALIZED (SELECT explicit,p,COUNT(*) AS facts FROM facts GROUP BY explicit,p) "
              "SELECT grouped.explicit,grouped.p,grouped.facts,terms.n AS predicate_n,terms.kind AS predicate_kind "
              "FROM grouped LEFT JOIN terms ON terms.id=grouped.p ORDER BY grouped.explicit,terms.n,grouped.p",
              0, NULL);
    } else {
        char sql[4096] = "WITH requested(n) AS (VALUES ";
        for (int i = 3; i < argc; ++i) strcat(sql, i == 3 ? "(?)" : ",(?)");
        strcat(sql,
               "), selected(id) AS MATERIALIZED (SELECT terms.id FROM terms JOIN requested ON requested.n=terms.n) "
               "SELECT f.g,f.explicit,s.n AS subject_n,p.n AS predicate_n,o.n AS object_n "
               "FROM selected JOIN facts f ON f.s=selected.id "
               "LEFT JOIN terms s ON s.id=f.s LEFT JOIN terms p ON p.id=f.p LEFT JOIN terms o ON o.id=f.o "
               "ORDER BY s.n,p.n,o.n,f.explicit,f.g");
        query(db, "sample_outgoing", sql, argc - 3, argv + 3);
    }
    if (sqlite3_exec(db, "COMMIT", NULL, NULL, NULL) != SQLITE_OK) fail(db, "finish read transaction");
    printf("]}\n");
    if (sqlite3_close(db) != SQLITE_OK) fail(NULL, "close readonly database");
    return 0;
}
