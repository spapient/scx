// scx 自研 GPU 内核宿主（Metal，ObjC++）：实现 scx-sem-mat 协议。
// 读一行 JSON 请求（特征矩阵 + 权重），Metal 跑 matvec，逐行输出居中分数与 done。
// 任何解析/设备/计算失败 → 非零退出，由 Rust 侧自动回落 CPU。

#import <Foundation/Foundation.h>
#import <Metal/Metal.h>

#include <string>
#include <vector>

// 从 json 中取 "key":[...] 括号内的浮点序列（请求由本项目 Rust 生成，结构固定）。
static bool numbers(const std::string& j, const std::string& key, std::vector<float>& out) {
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

static uint integer(const std::string& j, const std::string& key) {
    size_t p = j.find("\"" + key + "\":");
    if (p == std::string::npos) return 0;
    p += key.size() + 3;
    return static_cast<uint>(std::strtoul(j.c_str() + p, nullptr, 10));
}

int main() {
    @autoreleasepool {
        std::string req;
        // 单块请求为单行；读到 EOF 即结束。
        for (;;) {
            int c = std::getchar();
            if (c == EOF) break;
            if (c == '\n') break;
            req.push_back(static_cast<char>(c));
        }
        if (req.empty()) return 2;

        std::vector<float> x, w;
        if (!numbers(req, "x", x) || !numbers(req, "w", w)) return 2;
        uint rows = integer(req, "rows");
        uint dim = integer(req, "dim");
        if (rows == 0 || dim == 0 || w.size() != dim || x.size() != size_t(rows) * dim) return 2;

        id<MTLDevice> dev = MTLCreateSystemDefaultDevice();
        if (!dev) return 3;
        id<MTLCommandQueue> queue = [dev newCommandQueue];

        NSError* err = nil;
        // 显式环境变量优先（文件路径），否则用仓库相对路径；两者都按文件读取。
        const char* alt = std::getenv("SCX_METAL_SRC");
        NSString* path = alt ? [NSString stringWithCString:alt encoding:NSUTF8StringEncoding]
                             : @"gpu/metal.metal";
        NSString* src = [NSString stringWithContentsOfFile:path
                                                 encoding:NSUTF8StringEncoding error:&err];
        if (!src) {
            std::fprintf(stderr, "metal: source not found at %s\n", path.UTF8String);
            return 3;
        }
        MTLCompileOptions* opt = [MTLCompileOptions new];
        id<MTLLibrary> lib = [dev newLibraryWithSource:src options:opt error:&err];
        if (!lib) {
            std::fprintf(stderr, "metal: compile: %s\n",
                         err ? err.localizedDescription.UTF8String : "?");
            return 3;
        }
        id<MTLFunction> fn = [lib newFunctionWithName:@"scx_mat"];
        if (!fn) {
            std::fprintf(stderr, "metal: function scx_mat missing\n");
            return 3;
        }

        id<MTLBuffer> bx = [dev newBufferWithBytes:x.data() length:x.size() * sizeof(float)
                                           options:MTLResourceStorageModeShared];
        id<MTLBuffer> bw = [dev newBufferWithBytes:w.data() length:w.size() * sizeof(float)
                                           options:MTLResourceStorageModeShared];
        id<MTLBuffer> bo = [dev newBufferWithLength:rows * sizeof(float)
                                            options:MTLResourceStorageModeShared];
        id<MTLComputePipelineState> pso = [dev newComputePipelineStateWithFunction:fn error:&err];
        if (!pso) return 3;

        id<MTLCommandBuffer> cb = [queue commandBuffer];
        id<MTLComputeCommandEncoder> enc = [cb computeCommandEncoder];
        [enc setComputePipelineState:pso];
        [enc setBuffer:bx offset:0 atIndex:0];
        [enc setBuffer:bw offset:0 atIndex:1];
        [enc setBuffer:bo offset:0 atIndex:2];
        uint d = dim;
        [enc setBytes:&d length:sizeof(d) atIndex:3];
        MTLSize grid = MTLSizeMake(rows, 1, 1);
        NSUInteger t = pso.maxTotalThreadsPerThreadgroup;
        if (t > rows) t = rows;
        [enc dispatchThreads:grid threadsPerThreadgroup:MTLSizeMake(t, 1, 1)];
        [enc endEncoding];
        [cb commit];
        [cb waitUntilCompleted];
        if (cb.status != MTLCommandBufferStatusCompleted) return 3;

        const float* res = static_cast<const float*>(bo.contents);
        for (uint i = 0; i < rows; ++i) {
            if (!std::isfinite(res[i])) return 4;
            std::printf("%.9g\n", res[i]);
        }
        std::printf("{\"done\":true,\"n\":%u}\n", rows);
    }
    return 0;
}
