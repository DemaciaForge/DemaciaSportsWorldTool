#!/usr/bin/env python3
"""mitmproxy 抓包插件：只保留跑步相关接口，落成 JSONL。

用法：
    mitmdump -p 8080 -s tools/mitm_capture.py --set confdir=_mitmconf

输出：capture/run_capture.jsonl（每行一条）
  {"t": "...", "method": "...", "url": "...", "status": 200,
   "req_headers": {...}, "req_body": "...", "resp_body": "..."}

只记录白名单路径，其余直接放行不落盘，避免抓到无关流量。
"""
import json
import os
import time

from mitmproxy import ctx, http

OUT_DIR = os.environ.get("RUN_CAPTURE_DIR", "capture")
OUT_PATH = os.path.join(OUT_DIR, "run_capture.jsonl")

# 跑步链路上需要看清的接口
WANT = (
    "/runnings/save/record",      # 提交跑步记录
    "/runnings/records",          # 记录列表
    "/runnings/get_one_record",   # 单条详情
    "/runnings/appeal/",          # 申诉
    "/runModePolicy",             # 学校规则
    "/runnings/upload/originalSign",
    "/obs/temporary/url",         # OBS 预签名
    "/run/calendar/add",
    "/runnings/recordssummary",
    # 极验 GeeGuard：本地 token → 服务端 token 的兑换端点
    "client_report",
    "riskct.geetest.com",
    # 极验 GT4 滑块（登录用）
    "gcaptcha4.geetest.com",
)

MAX_BODY = 4 * 1024 * 1024  # 单条 body 上限 4 MB

# 极验本地 token 的 AES-256 密钥。
#
# 来源：libgtcore.so 的 .rodata 偏移 0x5807 起 32 字节，逐字节 XOR 0x6e。
# 原始字节 393d5f000c393f08... 异或后即本常量。已用黄金向量验证：
#   plain={"a":1} + random4=11223344
#   → R0VFAAUxMTIyMzM0NDAxNWFiZDdmAAAAECc8nwc1mlvLNbq5fbAGXQs=
GT_AES_KEY = b"WS1nbWQfMJ1merhqY5RsFZIxrJ/54JE="


def _trim(text: str) -> str:
    if text is None:
        return ""
    if len(text) > MAX_BODY:
        return text[:MAX_BODY] + "...<truncated>"
    return text


def _gee_decrypt(raw: bytes):
    """GEE 容器的 AES-256-CBC 解密。缺 cryptography 库时返回 None。"""
    try:
        from cryptography.hazmat.primitives import padding
        from cryptography.hazmat.primitives.ciphers import Cipher, algorithms, modes
    except ImportError:
        return None
    try:
        iv = raw[5:21]
        n = int.from_bytes(raw[21:25], "big")
        body = raw[25:25 + n]
        dec = Cipher(algorithms.AES(GT_AES_KEY), modes.CBC(iv)).decryptor()
        padded = dec.update(body) + dec.finalize()
        unp = padding.PKCS7(128).unpadder()
        return (unp.update(padded) + unp.finalize()).decode("utf-8", "replace")
    except Exception:
        return None


def _gee_shape(text: str) -> dict:
    """识别极验 token 形态，并（GEE 容器）解出明文关键字段。"""
    import base64
    import json as _json

    s = (text or "").strip()
    if not s:
        return {}
    out = {}
    if s.startswith("R0VF"):
        out["kind"] = "local_GEE"
    elif s.startswith("RzAy"):
        out["kind"] = "server_G02"
    else:
        return {}
    try:
        b = base64.b64decode(s)
    except Exception:
        return out
    out["b64_len"] = len(s)
    out["raw_len"] = len(b)
    if len(b) < 25 or b[:4] != b"GEE\x00":
        return out
    out["ver"] = b[4]
    out["iv"] = b[5:21].decode("ascii", "replace")
    out["body_len"] = int.from_bytes(b[21:25], "big")
    plain = _gee_decrypt(b)
    if plain is None:
        out["plain"] = "需要 cryptography 库才能解密"
        return out
    out["plain_len"] = len(plain)
    try:
        j = _json.loads(plain)
        out["plain_keys"] = sorted(j.keys())
        out["appId"] = j.get("appId")
        out["libVer"] = j.get("libVer")
        out["ts"] = j.get("ts")
        out["et"] = j.get("et")
        data = j.get("data") or {}
        out["dev_field_count"] = len((data.get("dev") or {}))
        out["dev_fields"] = sorted((data.get("dev") or {}).keys())
        out["gid"] = (data.get("gid") or {}).get("3.0")
        out["test"] = data.get("test")
        out["sign"] = j.get("sign")
    except Exception as e:
        out["plain_head"] = plain[:200]
        out["parse_error"] = str(e)
    return out


def _wanted(url: str) -> bool:
    return any(k in url for k in WANT)


def response(flow: http.HTTPFlow) -> None:
    if not _wanted(flow.request.pretty_url):
        return
    os.makedirs(OUT_DIR, exist_ok=True)
    rec = {
        "t": time.strftime("%Y-%m-%d %H:%M:%S"),
        "method": flow.request.method,
        "url": flow.request.pretty_url,
        "status": flow.response.status_code if flow.response else None,
        "req_headers": dict(flow.request.headers),
        "req_body": _trim(flow.request.get_text(strict=False)),
        "resp_body": _trim(flow.response.get_text(strict=False)) if flow.response else "",
    }
    # 极验 token 形态识别：直接告诉你真机发的是 GEE（本地降级）还是 G02（服务端）
    shp = _gee_shape(rec["req_body"])
    if shp:
        rec["gee_req"] = shp
    if "client_report" in flow.request.pretty_url and flow.response:
        try:
            j = json.loads(flow.response.get_text(strict=False) or "{}")
            tok = (j.get("data") or {}).get("gee_token", "")
            if tok:
                rec["gee_resp"] = _gee_shape(tok)
            rec["gee_resp_code"] = j.get("code")
            rec["gee_resp_msg"] = j.get("msg")
        except Exception:
            pass
    with open(OUT_PATH, "a", encoding="utf-8") as f:
        f.write(json.dumps(rec, ensure_ascii=False) + "\n")
    ctx.log.info("[capture] %s %s -> %s%s", flow.request.method,
                 flow.request.path.split("?")[0], rec["status"],
                 ("  [gee %s]" % rec["gee_req"]["kind"]) if shp else "")