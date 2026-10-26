//! 冻结的校准系数（`TokenMode::Calibrated` 口径）。
//!
//! 由 `tools/gen_token_calib.py` 对真实 tokenizer（cl100k_base）离线最小二乘回归
//! 生成后入 repo；本文件为生成产物，改 tokenizer 后重跑脚本再提交。当前值为
//! 「复现启发式」的占位系数（ASCII 4 字/tok、CJK 0.6 tok/字、other 2.5 字/tok、
//! 截距 +1），与 `crate::terms::estimate` 逐位一致。

#![forbid(unsafe_code)]

/// 线性系数：特征序 `[ascii_alnum, ascii_punct, ascii_space, cjk, other, lines]` + 截距。
pub const CALIB_COEFFS: [f64; 7] = [0.25, 0.25, 0.25, 0.6, 0.4, 0.0, 1.0];
