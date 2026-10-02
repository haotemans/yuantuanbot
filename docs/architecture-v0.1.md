# 云团（Cloud Agent）总体架构设计 V0.1

> 云团不是传统 Bot，而是一个具有人格、长期记忆、社会关系、工具能力、自主决策能力的长期运行 Agent。

---

# 一、核心理念

云团的目标：

一个长期运行的 AI Agent。

它具备：

- 人格
- 长期记忆
- 社会关系
- 工具能力
- 自主决策能力

核心原则：

- 外部像一个人
- 内部像一个操作系统

用户不应该看到内部执行过程。

不展示：

- Decision 过程
- LLM 调用链
- Tool 调用链
- Memory 查询
- Agent Loop

用户只看到：

- 自然聊天
- 必要反馈
- 最终结果

---

# 二、整体架构

```text
                     云团
                       |
    ┌──────────────────┴──────────────────┐
    Bot系统                             Agent系统
  （交流人格）                        （任务执行）
    |                                   |
长期记忆                              短期记忆
关系系统                              Task状态
人格系统                              工具调用
群聊上下文                            工作上下文
    └──────────────────┬──────────────────┘
                       |
                Decision 小脑
                       |
                     LLM
                       |
                Rust Runtime Kernel
```

---

# 三、Bot 系统（长期存在）

负责：

- 群聊
- 私聊
- 人格表现
- 社交关系
- 长期记忆

---

## 3.1 Identity 身份系统

目标：

认识"人"，而不是认识昵称。

结构：

```text
Person
person_id
|
Identity
QQ
Telegram
Discord
微信
```

支持：

- QQ
- Telegram
- Discord
- 微信等

昵称只是属性：

```text
小明
↓
明哥
↓
Ming
```

Identity 不因为昵称变化而变化。

---

## 3.2 Relationship 关系库

目标：

理解人与人之间的关系。

不是：

```text
A 是朋友
```

而是：

```text
A → B

信任:
0.8

熟悉:
0.7

来源:
A帮助B解决问题

时间:
2026-09-01
```

结构：

```text
Person

Relationship Edge

Relationship Event
```

保存：

- 谁帮助过谁
- 谁合作过
- 谁经常交流
- 谁之间关系变化

---

## 3.3 Personality 人格系统

人格不是一个 Prompt。

而是一套长期状态。

保存：

- 说话方式
- 主动程度
- 幽默程度
- 不同人的交流方式
- 长期行为特点

人格可以随着经历变化。

---

## 3.4 Long Memory 长期记忆

属于 Bot。

保存：

用户：

- 喜欢什么
- 正在做什么
- 技术偏好

例如：

```text
A喜欢Rust

A正在研究机器人

A喜欢技术讨论
```

云团经历：

```text
曾经帮助A完成机器人项目
```

不保存：

- 临时日志
- 工具输出
- 中间步骤

---

# 四、Agent 系统（负责执行）

Agent 是云团的执行能力。

例如：

用户：

> 帮我写一个网站

流程：

```text
Bot收到消息

↓

Decision判断

↓

创建Agent Task

↓

Agent执行

↓

结果返回Bot

↓

Bot回复用户
```

---

## 4.1 Agent Short Memory

工作记忆。

只服务当前任务。

例如：

```text
Task:
开发网站

目标:
完成登录系统

计划:

1. 前端
2. 后端
3. 测试

状态:
80%
```

任务结束：

- 清理
- 或归档

---

## 4.2 Task 系统

管理任务生命周期。

结构：

```text
Task

task_id

goal

state:
pending
running
finished
failed

context

artifacts
```

---

# 五、Capability Tool 系统

云团能力。

## 文件

```text
create_file
read_file
compress
send_file
```

## 编程

```text
write_code
run_code
debug
git
```

## 网络

```text
search
api
download
```

## 多媒体

```text
image
video
audio
```

---

# 六、Agent 与 Bot 边界

非常重要：

禁止：

```text
Agent直接聊天
```

正确：

```text
Agent

↓

返回结果

↓

Bot组织语言

↓

发送消息
```

例如：

Agent：

```text
完成:
project.zip

测试通过
```

Bot：

```text
好了，代码整理好了，压缩包发你。
```

---

# 七、Memory 架构

三层：

```text
              Memory
                 |
     ┌───────────┼─────────────┐
Long Memory   Working Memory   Archive
     Bot           Agent          历史
  人物关系        当前任务        完整轨迹
  用户习惯        工具状态        日志
    人格          中间结果        文件记录
```

---

# 八、Agent Archive

任务结束：

```text
任务结束

↓

Memory Consolidation

↓

1. 压缩归档

2. 提取长期信息
```

---

## 压缩归档

保存：

- 任务摘要
- 时间线
- 文件
- trace位置

例如：

```text
T001

完成机器人程序开发

文件:
xxx.zip
```

---

## 提取长期信息

进入 Bot Memory。

保存：

```text
A正在做机器人项目

A喜欢Rust

A有开发需求
```

不保存：

```text
gcc失败三次

修改xxx文件

工具日志
```

---

# 九、Knowledge Base 知识库

作用：

提供额外知识。

场景：

- 不联网
- 私有资料
- 用户文件
- 项目资料

流程：

```text
文件

↓

解析

↓

Embedding

↓

Vector DB

↓

RAG

↓

LLM
```

区别：

Memory：

> 关于人的信息

Knowledge Base：

> 关于知识的信息

---

# 十、State 状态系统

区别：

Memory：

过去发生什么。

State：

现在是什么状态。

保存：

- 当前群
- 当前用户
- 当前话题
- 当前任务
- 云团状态

---

# 十一、Event System

所有变化产生事件：

例如：

```text
收到消息

用户加入群

关系变化

任务完成

Memory更新
```

统一：

```text
Event Bus
```

---

# 十二、Context 系统

三类：

## Bot Context

聊天环境：

- 最近消息
- 当前说话人
- 群环境
- 关系

## Agent Context

任务环境：

- 目标
- 计划
- 工具结果
- 状态

## Decision Context

给小脑：

- 用户是谁
- 当前关系
- 消息类型
- 是否需要行动

---

# 十三、Decision 小脑

负责：

决策。

不是生成语言。

职责：

- 是否回复
- 是否执行任务
- 是否调用能力
- 是否保存记忆
- 是否@
- 回复长度
- 是否发送文件

输出：

```json
{
  "action": "reply",
  "mention": false,
  "use_agent": true,
  "tool": "coding"
}
```

---

# 十四、消息行为系统

目标：

像真人。

闲聊：

```text
哈哈确实
```

明确回应：

```text
@A 我看看
```

任务结果：

```text
摘要

+

文件

+

必要说明
```

避免刷屏。

---

# 十五、技术方向

## Runtime

Rust。

原因：

- 安全
- 并发
- 长时间运行
- 权限控制

## 数据库

初期：

SQLite

后期：

- PostgreSQL
- Redis
- Vector DB
- Object Storage

## WebUI

管理：

- 用户
- 关系图
- Memory
- Task
- Agent状态
- 调试

---

# 十六、最终定义

> 云团 = 一个由 Rust Runtime 驱动的长期运行 Agent，拥有 Bot 人格层、关系系统、长期记忆、知识库、Agent执行系统和 Decision 小脑；内部复杂运行，外部保持自然人格。

---

# 十七、第一阶段落地

1. Rust Runtime 骨架
2. AstrBot 二改迁移
3. Bot / Agent 分离
4. SQLite 数据层
5. Decision模型接入
6. Memory / Relationship基础版
7. Tool系统
8. WebUI管理端

---

# 十八、后续实现重点

- Runtime模块边界
- Bot ↔ Decision ↔ Agent接口协议
- Event Bus事件模型
- Context Builder
- Memory Schema
- Relationship Graph
- Task生命周期
- Tool Registry
- LLM Provider
- Decision输入输出Schema
- Trace / Archive
- QQ / Telegram适配层
- WebUI API

---

本文作为云团 V0.1 架构基线。

后续实现围绕此架构演进。
