#!/bin/bash
# yuantuan 服务器部署脚本:防 tar 解压重置 deploy/data 权限的雪崩
# 使用姿势:
#   1. 本地 tar czf /tmp/yuantuan-src.tar.gz Cargo.toml ... deploy/
#   2. scp /tmp/yuantuan-src.tar.gz ubuntu@<server>:/tmp/
#   3. ssh ubuntu@<server> 'bash ~/yuantuan/deploy.sh'
set -e

echo "① 解压新源码"
cd ~/yuantuan/src || (mkdir -p ~/yuantuan/src && cd ~/yuantuan/src)

# 保护现有 data 目录:tar 前备份到 /tmp(若 tar 不带 data 则保留现有)
if [ -d deploy/data ]; then
    rm -rf /tmp/yuantuan-data-backup
    cp -r deploy/data /tmp/yuantuan-data-backup
fi

tar -xzf /tmp/yuantuan-src.tar.gz -C ~/yuantuan/src/

# tar 包内不含 data,若原本有就保留;若新建则 chown
if [ ! -d deploy/data ]; then
    mkdir -p deploy/data
fi
if [ -d /tmp/yuantuan-data-backup ] && [ -z "$(ls -A deploy/data 2>/dev/null)" ]; then
    cp -r /tmp/yuantuan-data-backup/. deploy/data/
fi

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
