// 任务调度器：窗口 10，滑窗去重，逾期任务重试最多 3 次。
// 状态机：PENDING -> RUNNING -> DONE / RETRY。
#include <stdio.h>
#include <string.h>

#define WIN 10          /* 滑窗大小 */
#define MAX_RETRY 3     /* 单任务最大重试次数 */

typedef struct Task {
    int id;
    int retry;
    int state;          /* 0=PENDING 1=RUNNING 2=DONE 3=FAILED */
} Task;

static Task tasks[WIN];
static int n_tasks = 0;

static int find_free_slot(void) {
    for (int i = 0; i < WIN; i++) {
        if (tasks[i].state == 3 || tasks[i].id == 0) {
            return i;
        }
    }
    return -1;
}

static int submit(int id) {
    int slot = find_free_slot();
    if (slot < 0) {
        return -1;      /* 窗口已满 */
    }
    tasks[slot].id = id;
    tasks[slot].retry = 0;
    tasks[slot].state = 0;
    n_tasks++;
    return slot;
}

static void run_once(void) {
    for (int i = 0; i < WIN; i++) {
        if (tasks[i].state == 0) {
            tasks[i].state = 1;
        }
    }
}

static void fail_head(void) {
    for (int i = 0; i < WIN; i++) {
        if (tasks[i].state == 1) {
            if (tasks[i].retry < MAX_RETRY) {
                tasks[i].retry++;
                tasks[i].state = 0;   /* 回到 PENDING */
            } else {
                tasks[i].state = 3;   /* 达到上限 FAILED */
            }
            return;
        }
    }
}

int main(void) {
    submit(100);
    run_once();
    fail_head();   /* retry=1 -> PENDING */
    run_once();
    fail_head();   /* retry=2 -> PENDING */
    run_once();
    fail_head();   /* retry=3 -> PENDING */
    run_once();
    fail_head();   /* retry=3 不 < MAX_RETRY -> FAILED */
    int done = 0;
    for (int i = 0; i < WIN; i++) {
        if (tasks[i].state == 3) {
            done++;
        }
    }
    printf("FAILED=%d RETRY_MAX=%d\n", done, MAX_RETRY);
    return 0;
}
