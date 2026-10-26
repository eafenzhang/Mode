#!/usr/bin/env python3
"""mymode.cloud 官网的一次性 Cloudflare 配置（幂等，可重复执行）。

在 fnOS NAS 上以 eafen 用户执行（token 读取自 tunnel 应用配置，不出 NAS）：
    python3 setup-cloudflare.py

做两件事：
1. 隧道 ingress：在既有业务路由之前插入 mymode.cloud / www.mymode.cloud 两条规则，
   指向本机 127.0.0.1:8095（nginx 容器），catch-all 404 保持最后。
2. DNS：删除 apex/WWW 上旧的 A/AAAA/CNAME 记录（原 A 记录指向已失效源站），
   建 proxied CNAME 指向 <tunnel-id>.cfargotunnel.com（apex 依赖 Cloudflare 扁平化）。

不触碰 relay / agentgo / hub-cmd / gashub 等其他子域。
"""

import json
import sys
import urllib.error
import urllib.request

APP_CONFIG = "/vol1/@appconf/com.dustinky.tunnel/config.json"
TUNNEL_INFO = "/vol1/@appconf/com.dustinky.tunnel/tunnel_info.json"
ZONE_NAME = "mymode.cloud"
SITE_HOSTS = ("mymode.cloud", "www." + ZONE_NAME)
ORIGIN_SERVICE = "http://127.0.0.1:8095"
CF_API = "https://api.cloudflare.com/client/v4"


def cf(path, token, method="GET", body=None):
    req = urllib.request.Request(
        CF_API + path,
        data=json.dumps(body).encode() if body is not None else None,
        method=method,
        headers={"Authorization": "Bearer " + token, "Content-Type": "application/json"},
    )
    try:
        with urllib.request.urlopen(req, timeout=30) as resp:
            return json.load(resp)
    except urllib.error.HTTPError as e:
        return json.load(e)


def fail(context, result):
    print(f"[失败] {context}: {json.dumps(result.get('errors'), ensure_ascii=False)}", file=sys.stderr)
    sys.exit(1)


def main():
    app = json.load(open(APP_CONFIG))
    token = app["apiToken"]
    account_id = app["accountId"]
    tunnel_id = json.load(open(TUNNEL_INFO))["tunnelId"]

    # 1. zone
    z = cf(f"/zones?name={ZONE_NAME}", token)
    if not z.get("success") or not z["result"]:
        fail("查询 zone", z)
    zone_id = z["result"][0]["id"]
    print(f"[ok] zone {ZONE_NAME} = {zone_id}")

    # 2. tunnel ingress
    cfg_path = f"/accounts/{account_id}/cfd_tunnel/{tunnel_id}/configurations"
    cur = cf(cfg_path, token)
    if not cur.get("success"):
        fail("读取隧道配置", cur)
    ingress = (cur["result"].get("config") or {}).get("ingress", [])
    named = {r.get("hostname"): r.get("service") for r in ingress if r.get("hostname")}
    if all(named.get(h) == ORIGIN_SERVICE for h in SITE_HOSTS):
        print("[ok] 隧道 ingress 已包含官网路由，跳过")
    else:
        wanted = [{"hostname": h, "service": ORIGIN_SERVICE} for h in SITE_HOSTS]
        catchall = [r for r in ingress if not r.get("hostname")]
        others = [r for r in ingress if r.get("hostname") and r["hostname"] not in SITE_HOSTS]
        new_ingress = wanted + others + catchall
        put = cf(cfg_path, token, method="PUT", body={"config": {"ingress": new_ingress}})
        if not put.get("success"):
            fail("更新隧道 ingress", put)
        print("[ok] 隧道 ingress 已更新：")
        for i, r in enumerate(new_ingress):
            print(f"    {i} {r.get('hostname') or '(catch-all)'} -> {r['service']}")

    # 3. DNS：清理旧记录，建 proxied CNAME
    d = cf(f"/zones/{zone_id}/dns_records?per_page=100", token)
    if not d.get("success"):
        fail("读取 DNS 记录", d)
    target = f"{tunnel_id}.cfargotunnel.com"
    for rec in d["result"]:
        if rec["name"] not in SITE_HOSTS:
            continue
        if rec["type"] == "CNAME" and rec["content"] == target and rec["proxied"]:
            print(f"[ok] {rec['name']} CNAME 已指向隧道，跳过")
            continue
        if rec["type"] in ("A", "AAAA", "CNAME"):
            dele = cf(f"/zones/{zone_id}/dns_records/{rec['id']}", token, method="DELETE")
            if not dele.get("success"):
                fail(f"删除 {rec['name']} {rec['type']}", dele)
            print(f"[ok] 已删除旧记录 {rec['name']} {rec['type']} {rec['content']}")
    for name in SITE_HOSTS:
        body = {"type": "CNAME", "name": name, "content": target, "proxied": True, "ttl": 1}
        create = cf(f"/zones/{zone_id}/dns_records", token, method="POST", body=body)
        if not create.get("success"):
            # 并发或重复执行时记录可能已存在，重新核对
            chk = cf(f"/zones/{zone_id}/dns_records?name={name}&type=CNAME", token)
            if chk.get("success") and chk["result"]:
                print(f"[ok] {name} CNAME 已存在，跳过")
                continue
            fail(f"创建 {name} CNAME", create)
        print(f"[ok] 已创建 {name} CNAME -> {target} (proxied)")

    print("[完成] Cloudflare 配置结束；站点文件部署请运行 deploy.sh")


if __name__ == "__main__":
    main()
