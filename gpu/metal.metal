// scx 自研 GPU 内核（Metal）：特征矩阵 × 权重 → 居中分数。
// 一个 thread 对应一行：循环累加 dim 维点积，再做 2·sigmoid(z)−1。
// 仅承载确定性线性代数；特征由 Rust 侧 scx::semantic::feats 统一计算。

#include <metal_stdlib>
using namespace metal;

kernel void scx_mat(device const float* x       [[buffer(0)]],
                    device const float* w       [[buffer(1)]],
                    device float*       out     [[buffer(2)]],
                    constant uint&     dim      [[buffer(3)]],
                    uint              row      [[thread_position_in_grid]]) {
    float acc = 0.0f;
    uint base = uint(row) * dim;
    for (uint k = 0; k < dim; ++k) {
        acc += x[base + k] * w[k];
    }
    // 数值稳定 sigmoid：与 Rust 同分支口径，居中到 (−1,1)。
    float s;
    if (acc >= 0.0f) {
        s = 1.0f / (1.0f + metal::exp(-acc));
    } else {
        float e = metal::exp(acc);
        s = e / (1.0f + e);
    }
    out[row] = 2.0f * s - 1.0f;
}
