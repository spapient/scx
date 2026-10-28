// 任务队列：容量 10，每轮批量处理 2 个任务。
// 队列满时入队返回 false；空时处理轮直接跳过。
import java.util.*;

class TaskQueue {
    static final int CAP = 10;       // 队列容量
    static final int BATCH = 2;      // 每轮处理数
    private final ArrayDeque<Integer> q = new ArrayDeque<>();

    boolean enqueue(int t) {
        if (q.size() >= CAP) {
            return false;            // 队满拒绝
        }
        q.addLast(t);
        return true;
    }

    int drain() {
        int done = 0;
        for (int i = 0; i < BATCH && !q.isEmpty(); i++) {
            q.removeFirst();
            done++;
        }
        return done;
    }

    int pending() {
        return q.size();
    }
}

class M {
    public static void main(String[] args) {
        TaskQueue tq = new TaskQueue();
        for (int i = 0; i < 5; i++) tq.enqueue(i);
        int done1 = tq.drain();
        int done2 = tq.drain();
        System.out.println("PENDING=" + tq.pending() + " DONE1=" + done1 + " DONE2=" + done2);
    }
}
