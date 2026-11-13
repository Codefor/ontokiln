#include <sqlite3.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

static double now(void) { struct timespec t; clock_gettime(CLOCK_MONOTONIC,&t); return t.tv_sec+t.tv_nsec/1e9; }
static double deadline;
static int stop(void *unused) { (void)unused; return now()>deadline; }
static void quote(const unsigned char *s) {
    if (!s) { fputs("null",stdout); return; }
    putchar('"'); for (;*s;s++) { if (*s=='"' || *s=='\\') { putchar('\\'); putchar(*s); }
        else if (*s<32) printf("\\u%04x",*s); else putchar(*s); } putchar('"');
}
static void die(sqlite3 *db,const char *what) { fprintf(stderr,"%s: %s\n",what,db?sqlite3_errmsg(db):"invalid args"); exit(1); }
static sqlite3_stmt *prepare(sqlite3 *db,const char *sql,int argc,char **argv,int first) {
    sqlite3_stmt *s=NULL; if(sqlite3_prepare_v2(db,sql,-1,&s,NULL)!=SQLITE_OK)die(db,"prepare");
    for(int i=first;i<argc;i++)if(sqlite3_bind_int64(s,i-first+1,strtoll(argv[i],NULL,10))!=SQLITE_OK)die(db,"bind");
    return s;
}
int main(int argc,char **argv) {
    if(argc<4)die(NULL,"usage: probe DB plan|step SQL [integer bindings...]");
    sqlite3 *db=NULL; if(sqlite3_open_v2(argv[1],&db,(strcmp(argv[2],"create")==0 ? SQLITE_OPEN_READWRITE|SQLITE_OPEN_CREATE : SQLITE_OPEN_READONLY)|SQLITE_OPEN_NOMUTEX,NULL)!=SQLITE_OK)die(db,"open");
    if(strcmp(argv[2],"create")==0) { if(sqlite3_exec(db,argv[3],NULL,NULL,NULL)!=SQLITE_OK)die(db,"create fixture"); sqlite3_close(db); puts("{\"created\":true}"); return 0; }
    if(sqlite3_exec(db,"PRAGMA query_only=ON; PRAGMA mmap_size=0; PRAGMA cache_size=-4096; PRAGMA temp_store=FILE;",NULL,NULL,NULL)!=SQLITE_OK)die(db,"pragmas");
    deadline=now()+10; sqlite3_progress_handler(db,10000,stop,NULL);
    char *eqp=sqlite3_mprintf("EXPLAIN QUERY PLAN %s",argv[3]);
    sqlite3_stmt *s=prepare(db,eqp,argc,argv,4); sqlite3_free(eqp);
    printf("{\"sqlite_version\":");quote((const unsigned char*)sqlite3_libversion());
    printf(",\"sqlite_source_id\":");quote((const unsigned char*)sqlite3_sourceid());
    printf(",\"database_readonly\":%s",sqlite3_db_readonly(db,"main")==1?"true":"false");
    printf(",\"sql\":");quote((const unsigned char*)argv[3]); printf(",\"plan\":[");
    int code,first=1; while((code=sqlite3_step(s))==SQLITE_ROW) {if(!first)putchar(',');first=0;quote(sqlite3_column_text(s,3));}
    if(code!=SQLITE_DONE)die(db,"plan step"); sqlite3_finalize(s);printf("]");
    if(strcmp(argv[2],"step")==0) {
        s=prepare(db,argv[3],argc,argv,4); double t=now();int rows=0;
        printf(",\"rows\":[");
        while((code=sqlite3_step(s))==SQLITE_ROW) {
            if(rows<260) {if(rows)putchar(','); putchar('[');
                for(int i=0;i<sqlite3_column_count(s);i++) {if(i)putchar(',');switch(sqlite3_column_type(s,i)) {
                    case SQLITE_INTEGER:printf("%lld",(long long)sqlite3_column_int64(s,i));break;
                    case SQLITE_FLOAT:printf("%.17g",sqlite3_column_double(s,i));break;
                    case SQLITE_NULL:fputs("null",stdout);break;
                    default:quote(sqlite3_column_text(s,i));break;
                }}putchar(']');
            }rows++;
        }
        printf("],\"row_count\":%d,\"step_code\":%d,\"seconds\":%.9f,\"vm_steps\":%d,\"fullscan_steps\":%d,\"sorts\":%d",rows,code,now()-t,sqlite3_stmt_status(s,SQLITE_STMTSTATUS_VM_STEP,0),sqlite3_stmt_status(s,SQLITE_STMTSTATUS_FULLSCAN_STEP,0),sqlite3_stmt_status(s,SQLITE_STMTSTATUS_SORT,0));
        sqlite3_finalize(s);
    }
    printf("}\n");sqlite3_close(db);return 0;
}
