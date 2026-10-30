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
)

MAX_BODY = 4 * 1024 * 1024  # 单条 body 上限 4 MB


def _trim(text: str) -> str:
    if text is None:
        return ""
    if len(text) > MAX_BODY:
        return text[:MAX_BODY] + "...<truncated>"
    return text


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
    with open(OUT_PATH, "a", encoding="utf-8") as f:
        f.write(json.dumps(rec, ensure_ascii=False) + "\n")
    ctx.log.info("[capture] %s %s -> %s", flow.request.method,
                 flow.request.path.split("?")[0], rec["status"])