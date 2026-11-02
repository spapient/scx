// 4x4 矩阵模板：加法与标量乘。
// 模板参数 N 为矩阵维度，运行时不可改。
#include <iostream>

template <int N>
struct Mat {
    int a[N][N];

    Mat() {
        for (int i = 0; i < N; i++)
            for (int j = 0; j < N; j++)
                a[i][j] = 0;
    }

    void set(int i, int j, int v) { a[i][j] = v; }

    int trace() const {
        int s = 0;
        for (int i = 0; i < N; i++) s += a[i][i];
        return s;
    }
};

int main() {
    Mat<4> m;
    m.set(0, 0, 2);
    m.set(1, 1, 3);
    m.set(2, 2, 5);
    std::cout << "DIM=" << 4 << " TRACE=" << m.trace() << std::endl;
    return 0;
}
