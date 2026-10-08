#!/bin/bash
# yuantuan 服务器部署脚本:防 tar 解压重置 deploy/data 权限的雪崩
# 使用姿势:
#   1. 本地 tar czf /tmp/yuantuan-src.tar.gz Cargo.toml ... deploy/
#   2. scp /tmp/yuantuan-src.tar.gz ubuntu@<server>:/tmp/
#   3. ssh ubuntu@<server> 'bash ~/yuantuan/deploy.sh'
set -e

echo "① 解压新源码"
cd ~/yuantuan/src || (mkdir -p ~/yuantuan/src && cd ~/yuantuan/src)

# 保护现有 data 目录 + providers/config(tar 包内的 yuantuan-providers.toml 是仓库空模板,
# 用户在面板上填过会被解压覆盖 → 必须先备份后回灌)
if [ -d deploy/data ]; then
    rm -rf /tmp/yuantuan-data-backup
    cp -r deploy/data /tmp/yuantuan-data-backup
fi
[ -f deploy/yuantuan-providers.toml ] && cp deploy/yuantuan-providers.toml /tmp/yuantuan-providers.toml.bak
[ -f deploy/yuantuan-config.toml ]    && cp deploy/yuantuan-config.toml    /tmp/yuantuan-config.toml.bak

tar -xzf /tmp/yuantuan-src.tar.gz -C ~/yuantuan/src/

# tar 包内不含 data,若原本有就保留;若新建则 chown
if [ ! -d deploy/data ]; then
    mkdir -p deploy/data
fi
if [ -d /tmp/yuantuan-data-backup ] && [ -z "$(ls -A deploy/data 2>/dev/null)" ]; then
    cp -r /tmp/yuantuan-data-backup/. deploy/data/
fi
# 恢复用户在面板填过的 providers.toml / config.toml(若用户在面板上改过的话,
# 优先用用户改过的;tar 包里的模板只在用户从未改过时才生效)
[ -f /tmp/yuantuan-providers.toml.bak ] && cp /tmp/yuantuan-providers.toml.bak deploy/yuantuan-providers.toml
[ -f /tmp/yuantuan-config.toml.bak ]    && cp /tmp/yuantuan-config.toml.bak    deploy/yuantuan-config.toml

# 1000:1000 是容器 yuantuan user;宿主 ubuntu user 也是 1000
chown -R 1000:1000 deploy/data
chmod 755 deploy deploy/data

echo "② 重 build yuantuan 镜像"
docker compose -f deploy/compose.yml build yuantuan

echo "③ 重启容器"
docker compose -f deploy/compose.yml up -d yuantuan

echo "④ 状态确认"
sleep 5
docker ps --format "table {{.Names}}\t{{.Status}}" | grep -E "yuantuan|napcat"
