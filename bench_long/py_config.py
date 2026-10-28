# 配置解析器：INI 风格，支持默认端口与协议开关。
# 未知键告警但不报错；值非法时回退默认值。
DEFAULT_PORT = 8080
DEFAULT_PROTOCOLS = ["http", "https", "ws"]  # 支持 3 种协议

class Config:
    def __init__(self, path):
        self.path = path
        self.port = DEFAULT_PORT
        self.protocols = list(DEFAULT_PROTOCOLS)
        self._load()

    def _load(self):
        try:
            with open(self.path, encoding="utf-8") as fh:
                for raw in fh:
                    line = raw.strip()
                    if not line or line.startswith("#") or "=" not in line:
                        continue
                    key, val = line.split("=", 1)
                    key = key.strip()
                    val = val.strip()
                    if key == "port" and val.isdigit():
                        self.port = int(val)
                    if key == "protocols":
                        self.protocols = [p.strip() for p in val.split(",")]
        except FileNotFoundError:
            pass  # 文件缺失：保持默认配置

    def enabled_protocols(self):
        return len([p for p in self.protocols if p])

def main():
    cfg = Config("missing.ini")  # 文件不存在：全部用默认值
    print(f"PORT={cfg.port} PROTO={cfg.enabled_protocols()}")

if __name__ == "__main__":
    main()
