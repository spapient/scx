#!/usr/bin/env python3
"""生成 scx 的 SPDX 2.3 软件物料清单（SBOM）。

零依赖项目下，Cargo.lock 只有 scx 自身一个包；本脚本仍按 SPDX 2.3 输出完整字段，
便于 CI 产出与下游工具消费。输出确定性：相同 Cargo.lock 产生相同文档命名空间。

用法：
    python3 scripts/sbom.py --lock Cargo.lock --out sbom/scx.spdx.json
"""

import argparse
import hashlib
import json
import os
import re
import sys

SPDX_VERSION = "SPDX-2.3"
SPDX_ID = "SPDXRef-DOCUMENT"
TOOL = "scx-sbom"


def parse_lock(path):
    """极简 Cargo.lock 解析：只提取 [[package]] 段的 name/version/checksum。"""
    with open(path, encoding="utf-8") as handle:
        text = handle.read()
    blocks = re.split(r"\[\[package\]\]", text)[1:]
    pkgs = []
    for block in blocks:
        name = _field(block, "name")
        version = _field(block, "version")
        checksum = _field(block, "checksum")
        if name and version:
            pkgs.append({"name": name, "version": version, "checksum": checksum})
    return pkgs


def _field(block, key):
    m = re.search(r'^%s\s*=\s*"([^"]+)"' % key, block, re.M)
    return m.group(1) if m else None


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def build(pkgs, lock_bytes, root_name):
    ns = "https://github.com/spapient/scx/sbom/%s" % sha256(lock_bytes)
    packages = []
    for i, pkg in enumerate(pkgs):
        sid = "SPDXRef-Package-%s" % (root_name if i == 0 else pkg["name"])
        packages.append(
            {
                "SPDXID": sid,
                "name": pkg["name"],
                "versionInfo": pkg["version"],
                "downloadLocation": "https://crates.io/crates/%s/%s"
                % (pkg["name"], pkg["version"])
                if pkg["checksum"]
                else "NOASSERTION",
                "filesAnalyzed": False,
                "licenseConcluded": "MIT OR Apache-2.0",
                "licenseDeclared": "MIT OR Apache-2.0",
                "copyrightText": "NOASSERTION",
                "primaryPackagePurpose": "APPLICATION" if i == 0 else "LIBRARY",
            }
        )
        if pkg["checksum"]:
            packages[-1]["checksums"] = [
                {
                    "algorithm": "SHA256",
                    "checksumValue": pkg["checksum"].split(":")[-1],
                }
            ]
    rels = []
    root = packages[0]["SPDXID"]
    for pkg in packages[1:]:
        rels.append(
            {
                "spdxElementId": root,
                "relationshipType": "DEPENDS_ON",
                "relatedSpdxElement": pkg["SPDXID"],
            }
        )
    return {
        "spdxVersion": SPDX_VERSION,
        "dataLicense": "CC0-1.0",
        "SPDXID": SPDX_ID,
        "name": root_name,
        "documentNamespace": ns,
        "creationInfo": {
            "creators": ["Tool: %s" % TOOL],
            "created": "1970-01-01T00:00:00Z",
            "comment": "created 字段固定以保证可复现输出；实际生成时间见 CI 日志",
        },
        "packages": packages,
        "relationships": rels,
    }


def main(argv):
    ap = argparse.ArgumentParser()
    ap.add_argument("--lock", default="Cargo.lock")
    ap.add_argument("--out", default="sbom/scx.spdx.json")
    ap.add_argument("--name", default="scx")
    args = ap.parse_args(argv[1:])

    with open(args.lock, "rb") as handle:
        lock_bytes = handle.read()
    pkgs = parse_lock(args.lock)
    if not pkgs:
        print("Cargo.lock 未解析出任何 package", file=sys.stderr)
        return 1

    doc = build(pkgs, lock_bytes, args.name)
    out_dir = os.path.dirname(args.out)
    if out_dir:
        os.makedirs(out_dir, exist_ok=True)
    with open(args.out, "w", encoding="utf-8") as handle:
        json.dump(doc, handle, ensure_ascii=False, indent=2)
        handle.write("\n")
    print("SBOM: %s（%d 个包）" % (args.out, len(pkgs)))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
