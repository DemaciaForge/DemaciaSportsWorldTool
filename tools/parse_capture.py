#!/usr/bin/env python3
"""解析 mitm_capture.py 产出的 run_capture.jsonl，输出关键字段报告。

用法：
    python tools/parse_capture.py capture/run_capture.jsonl

关注点：
  1. 真实请求头（对比我们发的 4 个）
  2. headerSign 内层字段
  3. 提交体顶层字段 + geeToken / mockGps* 等
  4. allLocJson 逐点字段与实测统计
"""
import base64
import gzip
import io
import json
import statistics
import sys

sys.stdout = io.TextIOWrapper(sys.stdout.buffer, encoding="utf-8", errors="replace")

# App 实际发的 HTTP 头（OooO0OO.java:131-132 + HttpUtil.getHeaderMap）
APP_HTTP_HEADERS = {"Accept", "Content-Type", "appVersion", "headerSign"}
# 我们的客户端（api/client.rs envelope_request）
OURS_HTTP_HEADERS = {"Content-Type", "User-Agent", "appVersion", "headerSign"}


def load(path):
    rows = []
    with open(path, encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if line:
                rows.append(json.loads(line))
    return rows


def show_headers(rows):
    print("=" * 70)
    print("1) 真实请求头")
    print("=" * 70)
    seen = {}
    for r in rows:
        for k, v in r.get("req_headers", {}).items():
            seen.setdefault(k.lower(), set()).add(v if len(v) < 60 else v[:57] + "...")
    for k in sorted(seen):
        vals = list(seen[k])
        print("  %-18s %s" % (k, vals[0] if len(vals) == 1 else vals[:3]))
    got = set(k.lower() for k in seen)
    want = set(h.lower() for h in APP_HTTP_HEADERS)
    ours = set(h.lower() for h in OURS_HTTP_HEADERS)
    print()
    print("  App 需要但捕获缺失 : %s" % (sorted(want - got) or "无"))
    print("  我们缺、App 有     : %s" % (sorted((got & want) - ours) or "无"))
    print("  我们多、App 没有   : %s" % (sorted(ours - got) or "无"))


def find_save(rows):
    for r in rows:
        if "/runnings/save/record" in r.get("url", ""):
            return r
    return None


def try_parse_body(raw):
    """提交体通常被 NetSecKit 信封包住；先试明文 JSON。"""
    if not raw:
        return None
    try:
        return json.loads(raw)
    except Exception:
        return None


def show_submit(rows):
    print()
    print("=" * 70)
    print("2) 提交体（save/record）")
    print("=" * 70)
    rec = find_save(rows)
    if not rec:
        print("  未捕获到 save/record")
        return
    print("  状态: %s   时间: %s" % (rec.get("status"), rec.get("t")))
    body = try_parse_body(rec.get("req_body"))
    if body is None:
        print("  请求体不是明文 JSON（被信封加密），长度 %d 字节" % len(rec.get("req_body") or ""))
        print("  前 200 字符: %s" % (rec.get("req_body") or "")[:200])
        return
    keys = sorted(body.keys())
    print("  顶层字段 %d 个:" % len(keys))
    for k in keys:
        v = body[k]
        s = json.dumps(v, ensure_ascii=False) if not isinstance(v, str) else v
        if len(s) > 70:
            s = s[:67] + "..."
        print("    %-22s %s" % (k, s))
    print()
    print("  --- 关键字段对照 ---")
    for k in ("geeToken", "errorCode", "unauthorized", "mockGpsStrategy",
              "mockGpsProbability", "disToRealLocation", "faceCheck", "status"):
        if k in body:
            v = body[k]
            print("    %-22s %r" % (k, v if len(str(v)) < 80 else str(v)[:77] + "..."))
        else:
            print("    %-22s <缺失>" % k)


def parse_allloc(body):
    """allLocJson 可能是明文数组，也可能是 gzip+base64。"""
    raw = body.get("allLocJson")
    if not raw:
        return None
    if isinstance(raw, list):
        return raw
    s = str(raw).strip()
    if s.startswith("["):
        try:
            return json.loads(s)
        except Exception:
            return None
    try:
        blob = base64.b64decode(s)
        try:
            blob = gzip.decompress(blob)
        except Exception:
            pass
        return json.loads(blob.decode("utf-8", "replace"))
    except Exception:
        return None


def show_points(rows):
    print()
    print("=" * 70)
    print("3) 轨迹逐点（allLocJson）")
    print("=" * 70)
    rec = find_save(rows)
    if not rec:
        print("  未捕获到 save/record")
        return
    body = try_parse_body(rec.get("req_body"))
    if body is None:
        print("  请求体非明文，跳过")
        return
    pts = parse_allloc(body)
    if not pts:
        print("  allLocJson 解析失败")
        return
    print("  点数: %d   每点字段数: %d" % (len(pts), len(pts[0])))
    print("  字段: %s" % ",".join(sorted(pts[0].keys())))
    alt = [float(p.get("bdA") or 0) for p in pts]
    band = max(alt) - min(alt)
    med = statistics.median([abs(b - a) for a, b in zip(alt, alt[1:])] or [0])
    cnt = [int(p.get("count") or 0) for p in pts]
    print()
    print("  海拔 min=%.2f max=%.2f band=%.2f 中位Δ=%.3f" % (min(alt), max(alt), band, med))
    print("  count min=%d max=%d 均值=%.1f" % (min(cnt), max(cnt), sum(cnt) / len(cnt)))
    print()
    print("  真机基准（3 条确认记录）:")
    print("    band 5.00-11.78 中位Δ 0.31-0.48 count 53-79")
    print("  我们 v0.6.1（30 次干跑）:")
    print("    band 5.05-11.64 中位Δ 0.250-0.410 count 52-80")


def main():
    if len(sys.argv) < 2:
        print("用法: python tools/parse_capture.py <run_capture.jsonl>")
        return 1
    rows = load(sys.argv[1])
    print("共 %d 条记录" % len(rows))
    show_headers(rows)
    show_submit(rows)
    show_points(rows)
    return 0


if __name__ == "__main__":
    sys.exit(main())