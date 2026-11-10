// scx 自研 GPU 内核（CUDA）：实现 scx-sem-mat 协议，与 Metal 宿主同口径。
// 读一行 JSON 请求（特征矩阵 + 权重），CUDA 跑 matvec，逐行输出居中分数与 done。
// 任何失败 → 非零退出，由 Rust 侧自动回落 CPU。

#include <cuda_runtime.h>

#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <string>
#include <vector>

__global__ void scx_mat(const float* __restrict__ x,
                        const float* __restrict__ w,
                        float* __restrict__ out,
                        unsigned int dim) {
    unsigned int row = blockIdx.x * blockDim.x + threadIdx.x;
    float acc = 0.0f;
    unsigned int base = row * dim;
    for (unsigned int k = 0; k < dim; ++k) {
        acc += x[base + k] * w[k];
    }
    float s;
    if (acc >= 0.0f) {
        s = 1.0f / (1.0f + expf(-acc));
    } else {
        float e = expf(acc);
        s = e / (1.0f + e);
    }
    out[row] = 2.0f * s - 1.0f;
}

static bool numbers(const std::string& j, const std::string& key,
                    std::vector<float>& out) {
    std::string pat = "\"" + key + "\":[";
    size_t p = j.find(pat);
    if (p == std::string::npos) return false;
    p += pat.size();
    size_t end = j.find(']', p);
    if (end == std::string::npos) return false;
    out.clear();
    std::string cell;
    for (size_t i = p; i < end; ++i) {
        char c = j[i];
        if (c == ',') {
            if (!cell.empty()) out.push_back(std::stof(cell));
            cell.clear();
        } else if (!std::isspace(static_cast<unsigned char>(c))) {
            cell.push_back(c);
        }
    }
    if (!cell.empty()) out.push_back(std::stof(cell));
    return true;
}

static unsigned int integer(const std::string& j, const std::string& key) {
    size_t p = j.find("\"" + key + "\":");
    if (p == std::string::npos) return 0;
    p += key.size() + 3;
    return static_cast<unsigned int>(std::strtoul(j.c_str() + p, nullptr, 10));
}

#define CUDA_CHECK(call)                                                       \
    do {                                                                       \
        cudaError_t e = (call);                                                \
        if (e != cudaSuccess) {                                                \
            std::fprintf(stderr, "cuda: %s\n", cudaGetErrorString(e));         \
            return 3;                                                          \
        }                                                                      \
    } while (0)

int main() {
    std::string req;
    for (;;) {
        int c = std::getchar();
        if (c == EOF || c == '\n') break;
        req.push_back(static_cast<char>(c));
    }
    if (req.empty()) return 2;

    std::vector<float> x, w;
    if (!numbers(req, "x", x) || !numbers(req, "w", w)) return 2;
    unsigned int rows = integer(req, "rows");
    unsigned int dim = integer(req, "dim");
    if (rows == 0 || dim == 0 || w.size() != dim || x.size() != size_t(rows) * dim)
        return 2;

    int devices = 0;
    if (cudaGetDeviceCount(&devices) != cudaSuccess || devices == 0) return 3;
    CUDA_CHECK(cudaSetDevice(0));

    float *dx = nullptr, *dw = nullptr, *do_ = nullptr;
    CUDA_CHECK(cudaMalloc(&dx, x.size() * sizeof(float)));
    CUDA_CHECK(cudaMalloc(&dw, w.size() * sizeof(float)));
    CUDA_CHECK(cudaMalloc(&do_, rows * sizeof(float)));
    CUDA_CHECK(cudaMemcpy(dx, x.data(), x.size() * sizeof(float),
                          cudaMemcpyHostToDevice));
    CUDA_CHECK(cudaMemcpy(dw, w.data(), w.size() * sizeof(float),
                          cudaMemcpyHostToDevice));

    const int block = 128;
    int grid = (rows + block - 1) / block;
    scx_mat<<<grid, block>>>(dx, dw, do_, dim);
    CUDA_CHECK(cudaDeviceSynchronize());

    std::vector<float> host(rows);
    CUDA_CHECK(cudaMemcpy(host.data(), do_, rows * sizeof(float),
                          cudaMemcpyDeviceToHost));
    cudaFree(dx);
    cudaFree(dw);
    cudaFree(do_);

    for (float v : host) {
        if (!std::isfinite(v)) return 4;
        std::printf("%.9g\n", v);
    }
    std::printf("{\"done\":true,\"n\":%u}\n", rows);
    return 0;
}
