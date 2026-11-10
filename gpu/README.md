# gpu

scx 自研 GPU 内核：仅承载确定性线性代数（特征矩阵 × 权重 → 居中分数）。
特征由 Rust 侧 `scx::semantic::feats` 统一计算，内核不重复实现特征，保证
单一事实来源。协议为 `scx-sem-mat`（stdin 一行 JSON，stdout 逐行分数 + done），
任何失败非零退出，由 Rust 侧自动回落 CPU。发布包不含二进制，仅含源码，
本机按需编译，不做 exe 分发。

## 文件

| 文件 | 平台 | 说明 |
| --- | --- | --- |
| `metal.metal` | macOS | Metal 着色内核（matvec + 居中 sigmoid） |
| `metal.mm` | macOS | ObjC++ 宿主：解析请求、调度 Metal、输出分数 |
| `cuda.cu` | Windows/Linux | CUDA 内核 + 宿主，同协议同口径 |

## 构建

macOS（Metal）：

```sh
clang++ -std=c++17 -fobjc-arc -O2 gpu/metal.mm \
  -o ~/.local/share/scx/gpu/metal \
  -framework Metal -framework Foundation
```

Windows（CUDA，MSVC 宿主，先进入 vcvars 环境）：

```sh
nvcc -O2 -arch=sm_75 gpu/cuda.cu -o cuda.exe
```

Linux（CUDA）：

```sh
nvcc -O2 gpu/cuda.cu -o cuda
```

## 启用

内核路径经环境变量 `SCX_GPU_BIN` 提供，运行时加 `--sem-backend gpu`；
Metal 内核源码路径可用 `SCX_METAL_SRC` 显式指定。未设置或调用失败时
自动回落 `cpu`（确定性路径，golden 冻结基线只用 CPU）。

## 验证

CPU（f64）与 GPU（f32）对拍：随机特征矩阵上居中分数最大绝对误差
< 1e-3，压缩选线结果与 CPU 一致；GPU 不进入冻结基线。
