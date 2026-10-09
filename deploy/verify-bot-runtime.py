#!/usr/bin/env python3
"""Linux Docker smoke: migrate a read-only production snapshot and test SIGTERM.

No production container is started or changed. Test containers have no network,
no credentials and no QQ adapter. All writes are confined to temporary copies.
"""
import argparse
import json
import pathlib
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import time
import uuid


def docker(*args):
    return subprocess.check_output(["docker", *args], text=True).strip()


def verify(binary, source_db, image, phase):
    root = pathlib.Path(tempfile.mkdtemp(prefix="yuantuan-runtime-verify-"))
    name = "yuantuan-verify-" + uuid.uuid4().hex[:12]
    created = False
    try:
        (root / "data").mkdir()
        target = root / "data/yuantuan.db"
        with sqlite3.connect(source_db.as_uri() + "?mode=ro", uri=True) as src:
            with sqlite3.connect(target) as dest:
                src.backup(dest)
        with sqlite3.connect(target) as db:
            persona_count, active_count = db.execute("SELECT COUNT(*),COALESCE(SUM(active=1),0) FROM personality_versions").fetchone()
            expected_active = active_count if persona_count else 1
            before = {t: db.execute("SELECT COUNT(*) FROM " + t).fetchone()[0]
                      for t in ["persons", "messages", "long_memories"]}
            if phase == "startup":
                db.execute("INSERT INTO persons(person_id,display_name,first_seen,last_seen) VALUES ('p_verify','probe',1,1)")
                db.executemany("INSERT INTO messages(chat_id,chat_type,sender_pid,text,at_me,ts) VALUES ('123','private','p_verify','probe',1,?)",
                               [(int(time.time()),)] * 40)
                before["persons"] += 1
                before["messages"] += 40
        (root / "config.toml").write_text('''[napcat]
enabled=false
[consolidation]
enabled=false
[backup]
enabled=false
''')
        (root / "providers.toml").write_text(
            '[provider.mock]\nbase_url="http://127.0.0.1:9"\napi_key_env=""\n[roles]\ndecision={provider="mock",model="mock"}\n'
            if phase == "startup" else "[roles]\n")
        docker("run", "-d", "--name", name, "--network", "none", "--memory", "512m",
               "--cpus", "1", "--pids-limit", "128", "--user", str(root.stat().st_uid),
               "--mount", f"type=bind,source={root},target=/app",
               "--mount", f"type=bind,source={binary},target=/usr/local/bin/yuantuan,readonly",
               image)
        created = True
        deadline = time.monotonic() + 40
        while True:
            logs = docker("logs", name)
            ready = "Decision 模型调用失败" in logs if phase == "startup" else "WebUI 开始监听" in logs
            if ready:
                break
            if time.monotonic() > deadline:
                raise RuntimeError("test instance did not reach expected phase")
            time.sleep(0.1)
        if phase == "startup":
            assert "WebUI 开始监听" not in logs, "must stop during replay"
        started = time.monotonic()
        docker("stop", "--time", "10", name)
        elapsed = time.monotonic() - started
        state = json.loads(docker("inspect", name))[0]["State"]
        logs = docker("logs", name)
        assert state["ExitCode"] == 0, state
        assert "优雅停机完成" in logs
        assert "WAL checkpoint 完成" in logs
        with sqlite3.connect(target) as db:
            version = db.execute("PRAGMA user_version").fetchone()[0]
            assert version == 6, version
            assert db.execute("PRAGMA integrity_check").fetchone()[0] == "ok"
            after = {t: db.execute("SELECT COUNT(*) FROM " + t).fetchone()[0] for t in before}
            assert before == after, (before, after)
            assert db.execute("SELECT COUNT(*) FROM personality_versions WHERE active=1").fetchone()[0] == expected_active
        print(json.dumps({"phase": phase, "exit_code": 0, "stop_seconds": round(elapsed, 3),
                          "database_version": version, "preserved_rows": after,
                          "checkpoint": "ok", "integrity": "ok"}), flush=True)
    finally:
        if created:
            subprocess.run(["docker", "rm", "-f", name], check=True, stdout=subprocess.DEVNULL)
        shutil.rmtree(root)


if __name__ == "__main__":
    if sys.platform != "linux":
        raise SystemExit("Linux only")
    parser = argparse.ArgumentParser()
    parser.add_argument("binary", type=lambda p: pathlib.Path(p).resolve(strict=True))
    parser.add_argument("source_db", type=lambda p: pathlib.Path(p).resolve(strict=True))
    parser.add_argument("--image", default="yuantuan:local")
    args = parser.parse_args()
    for phase in ["ready", "startup"]:
        verify(args.binary, args.source_db, args.image, phase)
