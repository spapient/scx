# 发布产物的签名与验证

`scx` 的发布资产使用 [Sigstore cosign](https://docs.sigstore.dev/) 做**无密钥签名**
（keyless）。签名身份由 GitHub Actions 的 OIDC 令牌证明，签名记录写入 Sigstore 的
透明日志（Rekor），因此验证者不需要预先获得维护者的公钥。

对应 OpenSSF Best Practices silver 级的 `signed_releases` 与 `version_tags_signed` 条款。

## 发布包含哪些文件

| 文件 | 内容 |
| --- | --- |
| `scx-darwin-arm64` | macOS（Apple Silicon）二进制 |
| `scx-linux-x64` | Linux x86-64 静态链接二进制（musl static-pie） |
| `scx-win32-x64.exe` | Windows x86-64 二进制 |
| `SHA256SUMS.txt` | 上述三个资产的 SHA-256 校验和 |
| `<资产>.bundle` | 对应资产的 cosign 签名包（含证书与透明日志条目） |
| `SHA256SUMS.txt.bundle` | 校验和文件的签名包 |

## 为什么用无密钥签名

传统 GPG 签名要求维护者长期保管私钥，私钥泄漏即等于供应链失守。
Sigstore keyless 的做法是：签名时由 CI 的 OIDC 令牌向 Fulcio 申请一张**短时效证书**，
签名完成后私钥即销毁，签名证据写入 Rekor 透明日志。

验证方不需要信任某把长期公钥，只需核对：签名时的身份（OIDC 主体）
与仓库、工作流是否匹配。

## 验证步骤

前置：安装 cosign（`brew install cosign`，或从
<https://github.com/sigstore/cosign/releases> 下载）。

### 1. 校验完整性（先做这一步）

```bash
# 下载三个资产与校验和
gh release download v0.12.2 --pattern 'scx-*'

# 校验
shasum -a 256 -c SHA256SUMS.txt
```

### 2. 校验签名

```bash
cosign verify-blob \
  --bundle scx-darwin-arm64.bundle \
  --certificate-identity-regexp 'https://github\.com/spapient/scx/' \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com \
  scx-darwin-arm64
```

`--certificate-identity-regexp` 限定"签名必须来自本仓库的工作流"，
`--certificate-oidc-issuer` 限定"身份由 GitHub Actions 签发"。
两项同时满足时，输出 `Verified OK`。

### 3. 校验校验和文件本身

```bash
cosign verify-blob \
  --bundle SHA256SUMS.txt.bundle \
  --certificate-identity-regexp 'https://github\.com/spapient/scx/' \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com \
  SHA256SUMS.txt
```

三者都通过，可以认为资产未被篡改，且确实由本仓库的发布工作流产出。

## 可复现构建（补充验证手段）

零第三方 crate 依赖使第三方可以自行重建并比对。已验证：

```bash
CARGO_TARGET_DIR=/tmp/rep1 cargo build --release --locked
CARGO_TARGET_DIR=/tmp/rep2 cargo build --release --locked
shasum -a 256 /tmp/rep1/release/scx /tmp/rep2/release/scx
```

两次构建的 SHA-256 相同（同机、同工具链条件下）。
跨机器重建可能因工具链版本或链接器差异产生不同哈希，
此时应以签名与 `SHA256SUMS.txt` 为准。

## 签名失败时怎么办

发布工作流失败不会撤回已上传的资产，但**不应**把无签名的资产当作已签名发布。
维护者应在修复后重新运行工作流（`workflow_dispatch` 指定标签），
或用 `cosign sign-blob` 本地补签后手工 `gh release upload`。

## 尚未覆盖的部分

- 未生成 SLSA provenance attestation（`actions/attest-build-provenance`）；
  cosign 的 Rekor 记录已提供近似强度的"谁构建了什么"证据，
  但不是 SLSA Build 级别认证。
- 未对 git 标签做签名（`version_tags_signed` 仍为建议项）。
