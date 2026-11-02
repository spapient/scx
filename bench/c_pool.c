/*
 * 固定块内存池：1024 块，每块 32 字节。
 * 分配失败返回 NULL；释放后块回收到空闲链表。
 */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define POOL_BLOCKS 1024   /* 池内总块数 */
#define BLOCK_SIZE  32     /* 单块字节数 */

typedef struct Pool {
    char *mem;              /* 连续内存区 */
    int free_head;          /* 空闲链表头 */
    int next[POOL_BLOCKS];  /* 每块的下一个空闲块索引 */
} Pool;

static Pool pool;

static void pool_init(void) {
    pool.mem = calloc(POOL_BLOCKS, BLOCK_SIZE);
    pool.free_head = 0;
    for (int i = 0; i < POOL_BLOCKS; i++) {
        pool.next[i] = (i + 1 < POOL_BLOCKS) ? i + 1 : -1;
    }
}

static void *pool_alloc(void) {
    int b = pool.free_head;
    if (b < 0) {
        return NULL;        /* 池耗尽 */
    }
    pool.free_head = pool.next[b];
    return pool.mem + (size_t)b * BLOCK_SIZE;
}

static void pool_free(void *p) {
    if (!p) {
        return;
    }
    size_t off = (char *)p - pool.mem;
    int b = (int)(off / BLOCK_SIZE);
    pool.next[b] = pool.free_head;
    pool.free_head = b;
}

int main(void) {
    pool_init();
    void *p1 = pool_alloc();
    void *p2 = pool_alloc();
    pool_free(p1);
    void *p3 = pool_alloc();
    printf("TOTAL=%d USED=%d\n", POOL_BLOCKS, (int)((char *)p3 - (char *)pool.mem) / BLOCK_SIZE + 1);
    return 0;
}
