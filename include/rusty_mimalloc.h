/* rusty_alloc — mimalloc v2.4.5-compatible C interface (implemented subset).
   Link against rusty_alloc_ffi (cdylib/staticlib). Symbols use mimalloc's
   exact names; this header mirrors include/mimalloc.h for the functions that
   exist as of M7. Convenience macros are carried over verbatim. */
#pragma once
#ifndef RUSTY_MIMALLOC_H
#define RUSTY_MIMALLOC_H
#define MI_MALLOC_VERSION 20405

#include <stddef.h>
#include <stdbool.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* version / lifecycle */
int  mi_version(void);
void mi_collect(bool force);
void mi_thread_init(void);
void mi_thread_done(void);
void mi_process_init(void);
void mi_process_done(void);
void mi_thread_set_in_threadpool(void);

/* standard + extended (§5.1/5.2) */
void* mi_malloc(size_t size);
void* mi_calloc(size_t count, size_t size);
void* mi_realloc(void* p, size_t newsize);
void* mi_expand(void* p, size_t newsize);
void  mi_free(void* p);
void  mi_free_small(void* p);
char* mi_strdup(const char* s);
char* mi_strndup(const char* s, size_t n);
char* mi_realpath(const char* fname, char* resolved_name);
void* mi_malloc_small(size_t size);
void* mi_zalloc_small(size_t size);
void* mi_zalloc(size_t size);
void* mi_mallocn(size_t count, size_t size);
void* mi_reallocn(void* p, size_t count, size_t size);
void* mi_reallocf(void* p, size_t newsize);
size_t mi_usable_size(const void* p);
size_t mi_good_size(size_t size);

/* aligned (§5.4) + zero-preserving (§5.7) + u* (§5.5) */
void* mi_malloc_aligned(size_t size, size_t alignment);
void* mi_malloc_aligned_at(size_t size, size_t alignment, size_t offset);
void* mi_zalloc_aligned(size_t size, size_t alignment);
void* mi_zalloc_aligned_at(size_t size, size_t alignment, size_t offset);
void* mi_calloc_aligned(size_t count, size_t size, size_t alignment);
void* mi_calloc_aligned_at(size_t count, size_t size, size_t alignment, size_t offset);
void* mi_realloc_aligned(void* p, size_t newsize, size_t alignment);
void* mi_realloc_aligned_at(void* p, size_t newsize, size_t alignment, size_t offset);
void* mi_rezalloc(void* p, size_t newsize);
void* mi_recalloc(void* p, size_t newcount, size_t size);
void* mi_rezalloc_aligned(void* p, size_t newsize, size_t alignment);
void* mi_rezalloc_aligned_at(void* p, size_t newsize, size_t alignment, size_t offset);
void* mi_recalloc_aligned(void* p, size_t newcount, size_t size, size_t alignment);
void* mi_recalloc_aligned_at(void* p, size_t newcount, size_t size, size_t alignment, size_t offset);
void* mi_umalloc(size_t size, size_t* block_size);
void* mi_ucalloc(size_t count, size_t size, size_t* block_size);
void* mi_urealloc(void* p, size_t newsize, size_t* block_size_pre, size_t* block_size_post);
void  mi_ufree(void* p, size_t* block_size);
void* mi_umalloc_aligned(size_t size, size_t alignment, size_t* block_size);
void* mi_uzalloc_aligned(size_t size, size_t alignment, size_t* block_size);
void* mi_umalloc_small(size_t size, size_t* block_size);
void* mi_uzalloc_small(size_t size, size_t* block_size);

/* heaps (§5.6) */
struct mi_heap_s; typedef struct mi_heap_s mi_heap_t;
mi_heap_t* mi_heap_new(void);
mi_heap_t* mi_heap_new_ex(int heap_tag, bool allow_destroy, int arena_id);
mi_heap_t* mi_heap_new_in_arena(int arena_id);
void       mi_heap_delete(mi_heap_t* heap);
void       mi_heap_destroy(mi_heap_t* heap);
mi_heap_t* mi_heap_set_default(mi_heap_t* heap);
mi_heap_t* mi_heap_get_default(void);
mi_heap_t* mi_heap_get_backing(void);
void       mi_heap_collect(mi_heap_t* heap, bool force);
void* mi_heap_malloc(mi_heap_t* heap, size_t size);
void* mi_heap_zalloc(mi_heap_t* heap, size_t size);
void* mi_heap_calloc(mi_heap_t* heap, size_t count, size_t size);
void* mi_heap_mallocn(mi_heap_t* heap, size_t count, size_t size);
void* mi_heap_malloc_small(mi_heap_t* heap, size_t size);
void* mi_heap_zalloc_small(mi_heap_t* heap, size_t size);
void* mi_heap_realloc(mi_heap_t* heap, void* p, size_t newsize);
void* mi_heap_reallocn(mi_heap_t* heap, void* p, size_t count, size_t size);
void* mi_heap_reallocf(mi_heap_t* heap, void* p, size_t newsize);
char* mi_heap_strdup(mi_heap_t* heap, const char* s);
char* mi_heap_strndup(mi_heap_t* heap, const char* s, size_t n);
void* mi_heap_malloc_aligned(mi_heap_t* heap, size_t size, size_t alignment);
void* mi_heap_malloc_aligned_at(mi_heap_t* heap, size_t size, size_t alignment, size_t offset);
void* mi_heap_zalloc_aligned(mi_heap_t* heap, size_t size, size_t alignment);
void* mi_heap_zalloc_aligned_at(mi_heap_t* heap, size_t size, size_t alignment, size_t offset);
void* mi_heap_calloc_aligned(mi_heap_t* heap, size_t count, size_t size, size_t alignment);
void* mi_heap_calloc_aligned_at(mi_heap_t* heap, size_t count, size_t size, size_t alignment, size_t offset);
void* mi_heap_realloc_aligned(mi_heap_t* heap, void* p, size_t newsize, size_t alignment);
void* mi_heap_realloc_aligned_at(mi_heap_t* heap, void* p, size_t newsize, size_t alignment, size_t offset);
void* mi_heap_rezalloc(mi_heap_t* heap, void* p, size_t newsize);
void* mi_heap_recalloc(mi_heap_t* heap, void* p, size_t newcount, size_t size);
void* mi_heap_rezalloc_aligned(mi_heap_t* heap, void* p, size_t newsize, size_t alignment);
void* mi_heap_rezalloc_aligned_at(mi_heap_t* heap, void* p, size_t newsize, size_t alignment, size_t offset);
void* mi_heap_recalloc_aligned(mi_heap_t* heap, void* p, size_t newcount, size_t size, size_t alignment);
void* mi_heap_recalloc_aligned_at(mi_heap_t* heap, void* p, size_t newcount, size_t size, size_t alignment, size_t offset);
void* mi_heap_alloc_new(mi_heap_t* heap, size_t size);
void* mi_heap_alloc_new_n(mi_heap_t* heap, size_t count, size_t size);

/* analysis (§5.8) */
bool mi_heap_contains_block(mi_heap_t* heap, const void* p);
bool mi_heap_check_owned(mi_heap_t* heap, const void* p);
bool mi_check_owned(const void* p);
typedef struct mi_heap_area_s {
  void*  blocks; size_t reserved; size_t committed; size_t used;
  size_t block_size; size_t full_block_size; int heap_tag;
} mi_heap_area_t;
typedef bool (mi_block_visit_fun)(const mi_heap_t* heap, const mi_heap_area_t* area, void* block, size_t block_size, void* arg);
bool mi_heap_visit_blocks(const mi_heap_t* heap, bool visit_blocks, mi_block_visit_fun* visitor, void* arg);
bool mi_is_in_heap_region(const void* p);
bool mi_is_redirected(void);
bool mi_unsafe_heap_page_is_under_utilized(mi_heap_t* heap, void* p, size_t perc_threshold);

/* arenas + subprocs (§5.9) */
typedef int mi_arena_id_t;
int  mi_reserve_os_memory(size_t size, bool commit, bool allow_large);
int  mi_reserve_os_memory_ex(size_t size, bool commit, bool allow_large, bool exclusive, mi_arena_id_t* arena_id);
bool mi_manage_os_memory(void* start, size_t size, bool is_committed, bool is_large, bool is_zero, int numa_node);
bool mi_manage_os_memory_ex(void* start, size_t size, bool is_committed, bool is_large, bool is_zero, int numa_node, bool exclusive, mi_arena_id_t* arena_id);
void* mi_arena_area(mi_arena_id_t arena_id, size_t* size);
int  mi_reserve_huge_os_pages_at(size_t pages, int numa_node, size_t timeout_msecs);
int  mi_reserve_huge_os_pages_at_ex(size_t pages, int numa_node, size_t timeout_msecs, bool exclusive, mi_arena_id_t* arena_id);
int  mi_reserve_huge_os_pages_interleave(size_t pages, size_t numa_nodes, size_t timeout_msecs);
int  mi_reserve_huge_os_pages(size_t pages, double max_secs, size_t* pages_reserved);
void mi_debug_show_arenas(void);
void mi_arenas_print(void);
void mi_collect_reduce(size_t target_thread_owned);
typedef void* mi_subproc_id_t;
mi_subproc_id_t mi_subproc_main(void);
mi_subproc_id_t mi_subproc_new(void);
void mi_subproc_delete(mi_subproc_id_t subproc);
void mi_subproc_add_current_thread(mi_subproc_id_t subproc);
bool mi_abandoned_visit_blocks(mi_subproc_id_t subproc_id, int heap_tag, bool visit_blocks, mi_block_visit_fun* visitor, void* arg);

/* options (§5.10) — indices are ABI-compatible with mimalloc v2.4.5 */
typedef int mi_option_t;
bool mi_option_is_enabled(mi_option_t option);
void mi_option_enable(mi_option_t option);
void mi_option_disable(mi_option_t option);
void mi_option_set_enabled(mi_option_t option, bool enable);
void mi_option_set_enabled_default(mi_option_t option, bool enable);
long mi_option_get(mi_option_t option);
long mi_option_get_clamp(mi_option_t option, long min, long max);
size_t mi_option_get_size(mi_option_t option);
void mi_option_set(mi_option_t option, long value);
void mi_option_set_default(mi_option_t option, long value);
void mi_options_print(void);

/* stats + hooks (§5.3) */
typedef void (mi_deferred_free_fun)(bool force, unsigned long long heartbeat, void* arg);
typedef void (mi_output_fun)(const char* msg, void* arg);
typedef void (mi_error_fun)(int err, void* arg);
void mi_register_deferred_free(mi_deferred_free_fun* deferred_free, void* arg);
void mi_register_output(mi_output_fun* out, void* arg);
void mi_register_error(mi_error_fun* fun, void* arg);
void mi_stats_reset(void);
void mi_stats_merge(void);
void mi_stats_print(void* out);
void mi_stats_print_out(mi_output_fun* out, void* arg);
void mi_thread_stats_print_out(mi_output_fun* out, void* arg);
void mi_process_info(size_t* elapsed_msecs, size_t* user_msecs, size_t* system_msecs, size_t* current_rss, size_t* peak_rss, size_t* current_commit, size_t* peak_commit, size_t* page_faults);

/* posix/compat (§5.11) + C++ new (§5.12) */
void  mi_cfree(void* p);
void* mi__expand(void* p, size_t newsize);
size_t mi_malloc_size(const void* p);
size_t mi_malloc_good_size(size_t size);
size_t mi_malloc_usable_size(const void* p);
int   mi_posix_memalign(void** p, size_t alignment, size_t size);
void* mi_memalign(size_t alignment, size_t size);
void* mi_valloc(size_t size);
void* mi_pvalloc(size_t size);
void* mi_aligned_alloc(size_t alignment, size_t size);
void* mi_reallocarray(void* p, size_t count, size_t size);
int   mi_reallocarr(void* ptrp, size_t count, size_t size);
void* mi_aligned_recalloc(void* p, size_t newcount, size_t size, size_t alignment);
void* mi_aligned_offset_recalloc(void* p, size_t newcount, size_t size, size_t alignment, size_t offset);
void  mi_free_size(void* p, size_t size);
void  mi_free_size_aligned(void* p, size_t size, size_t alignment);
void  mi_free_aligned(void* p, size_t alignment);
int   mi_dupenv_s(char** buf, size_t* size, const char* name);
int   mi_wdupenv_s(unsigned short** buf, size_t* size, const unsigned short* name);
unsigned short* mi_wcsdup(const unsigned short* s);
unsigned char* mi_mbsdup(const unsigned char* s);
void* mi_new(size_t size);
void* mi_new_aligned(size_t size, size_t alignment);
void* mi_new_nothrow(size_t size);
void* mi_new_aligned_nothrow(size_t size, size_t alignment);
void* mi_new_n(size_t count, size_t size);
void* mi_new_realloc(void* p, size_t newsize);
void* mi_new_reallocn(void* p, size_t newcount, size_t size);

/* convenience macros (verbatim) */
#define mi_malloc_tp(tp)            ((tp*)mi_malloc(sizeof(tp)))
#define mi_zalloc_tp(tp)            ((tp*)mi_zalloc(sizeof(tp)))
#define mi_calloc_tp(tp,n)          ((tp*)mi_calloc(n,sizeof(tp)))
#define mi_mallocn_tp(tp,n)         ((tp*)mi_mallocn(n,sizeof(tp)))
#define mi_reallocn_tp(p,tp,n)      ((tp*)mi_reallocn(p,n,sizeof(tp)))
#define mi_recalloc_tp(p,tp,n)      ((tp*)mi_recalloc(p,n,sizeof(tp)))
#define mi_heap_malloc_tp(hp,tp)    ((tp*)mi_heap_malloc(hp,sizeof(tp)))
#define mi_heap_zalloc_tp(hp,tp)    ((tp*)mi_heap_zalloc(hp,sizeof(tp)))
#define mi_heap_calloc_tp(hp,tp,n)  ((tp*)mi_heap_calloc(hp,n,sizeof(tp)))

#ifdef __cplusplus
}
#endif
#endif /* RUSTY_MIMALLOC_H */
