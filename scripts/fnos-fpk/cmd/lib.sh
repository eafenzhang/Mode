#!/bin/bash
# fnOS 生命周期脚本共享函数（被 main 与各回调 source）。
# 唯一所有者约束见 docs/specs/fnos-fpk-packaging.md：
#   令牌 → etc/token，凭据密钥 → etc/credential-secret，均由本文件内函数生成。

MODE_APP_PORT_DEFAULT=3030

mode_log() {
    echo "$(date '+%Y-%m-%d %H:%M:%S') - $1" >> "${TRIM_PKGVAR}/info.log" 2>/dev/null || true
}

# 失败出口：日志 + 用户可见消息（TRIM_TEMP_LOGFILE 由 fnOS 收集展示）后退出 1。
mode_fail() {
    mode_log "ERROR: $1"
    if [ -n "${TRIM_TEMP_LOGFILE:-}" ]; then
        printf 'Mode: %s\n' "$1" >> "$TRIM_TEMP_LOGFILE" 2>/dev/null || true
    fi
    exit 1
}

mode_token_file() {
    printf '%s/token' "${TRIM_PKGETC}"
}

# 生成/读取访问令牌（40 位字母数字，0600 落盘）。
mode_ensure_token() {
    local file
    file="$(mode_token_file)"
    if [ ! -s "$file" ]; then
        umask 077
        LC_ALL=C tr -dc 'A-Za-z0-9' < /dev/urandom 2>/dev/null | head -c 40 > "$file" || return 1
        chmod 600 "$file" 2>/dev/null || true
    fi
    cat "$file"
}

mode_credential_secret_file() {
    printf '%s/credential-secret' "${TRIM_PKGETC}"
}

# 生成/读取凭据加密密钥（48 位字母数字，0600 落盘）；key 丢失会导致已存凭据无法解密。
mode_ensure_credential_secret() {
    local file
    file="$(mode_credential_secret_file)"
    if [ ! -s "$file" ]; then
        umask 077
        LC_ALL=C tr -dc 'A-Za-z0-9' < /dev/urandom 2>/dev/null | head -c 48 > "$file" || return 1
        chmod 600 "$file" 2>/dev/null || true
    fi
    cat "$file"
}

# Windows 打包的 fpk 没有执行位（fnpack 行为），安装期在此补齐：运行用户需要能执行内置 Node 与入口。
mode_fix_permissions() {
    local dest="${TRIM_APPDEST:-}"
    if [ -z "$dest" ] || [ ! -d "$dest" ]; then
        return 0
    fi
    if [ -n "${TRIM_USERNAME:-}" ]; then
        chown -R "${TRIM_USERNAME}:${TRIM_GROUPNAME:-$TRIM_USERNAME}" "$dest" 2>/dev/null || true
    fi
    chmod 755 "$dest/runtime/bin/node" 2>/dev/null || true
    chmod 755 "$dest/bin/mode.mjs" 2>/dev/null || true
}

# 把令牌写入应用入口 URL（app/ui/config 的 "url": "/?token=…"）。
# 升级会整体替换应用目录、令牌行恢复为 __MODE_TOKEN__ 占位符，故每次安装/升级/启动都要重写。
mode_patch_ui_config() {
    local token="$1"
    local candidate ui_config=""
    for candidate in \
        "${TRIM_APPDEST}/ui/config" \
        "/var/apps/${TRIM_APPNAME}/app/ui/config" \
        "/var/apps/${TRIM_APPNAME}/ui/config"; do
        if [ -f "$candidate" ]; then
            ui_config="$candidate"
            break
        fi
    done
    if [ -z "$ui_config" ]; then
        mode_log "WARN: 未找到 app/ui/config，跳过入口令牌写入"
        return 1
    fi
    sed -i "s/token=[A-Za-z0-9_]*/token=${token}/" "$ui_config" 2>/dev/null || return 1
    return 0
}

mode_detect_ip() {
    local ip=""
    if command -v hostname >/dev/null 2>&1; then
        ip="$(hostname -I 2>/dev/null | awk '{print $1}')"
    fi
    if [ -z "$ip" ] && command -v ip >/dev/null 2>&1; then
        ip="$(ip route get 1.1.1.1 2>/dev/null | awk '{ for (i = 1; i <= NF; i++) if ($i == "src") { print $(i + 1); exit } }')"
    fi
    printf '%s' "${ip:-127.0.0.1}"
}

mode_access_url() {
    local port="${1:-${TRIM_SERVICE_PORT:-$MODE_APP_PORT_DEFAULT}}"
    local token
    token="$(cat "$(mode_token_file)" 2>/dev/null)"
    printf 'http://%s:%s/?token=%s' "$(mode_detect_ip)" "$port" "$token"
}

mode_write_access_url_file() {
    printf '%s\n' "$1" > "${TRIM_PKGETC}/access-url.txt" 2>/dev/null || true
}

# 安装/升级/配置回调共用主体：令牌与密钥、权限修正、入口令牌、访问地址、用户可见提示。
mode_post_install() {
    local banner="$1"
    mkdir -p "${TRIM_PKGETC}" "${TRIM_PKGVAR}" 2>/dev/null || true

    local token url
    token="$(mode_ensure_token)" || mode_fail "无法生成访问令牌"
    mode_ensure_credential_secret >/dev/null || mode_fail "无法生成凭据密钥"

    mode_fix_permissions
    if [ -n "${TRIM_USERNAME:-}" ]; then
        chown -R "${TRIM_USERNAME}:${TRIM_GROUPNAME:-$TRIM_USERNAME}" "${TRIM_PKGETC}" "${TRIM_PKGVAR}" 2>/dev/null || true
    fi

    if ! mode_patch_ui_config "$token"; then
        mode_log "WARN: 入口令牌写入失败，请改用 etc/access-url.txt 中的地址访问"
    fi
    url="$(mode_access_url)"
    mode_write_access_url_file "$url"
    mode_log "${banner}：${url}"

    if [ -n "${TRIM_TEMP_LOGFILE:-}" ]; then
        {
            printf '%s\n' "$banner"
            printf '访问地址（含访问令牌，请妥善保管）：%s\n' "$url"
            printf '令牌文件：%s\n' "$(mode_token_file)"
        } >> "$TRIM_TEMP_LOGFILE" 2>/dev/null || true
    fi
}
