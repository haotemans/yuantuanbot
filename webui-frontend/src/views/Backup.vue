<template>
  <n-card title="备份 / 日志" style="max-width: 620px" size="small">
    <n-alert type="info">
      备份为运维层定时任务（精炼数据优先推 GitHub 私有仓，原始流水可选加密压缩），
      数据集中于 <code>data/</code> 目录（含 backups/ 暂存）——实现排期在后续施工单。
    </n-alert>
    <n-descriptions :column="1" size="small" style="margin-top: 12px">
      <n-descriptions-item label="备份目录">data/backups/（暂存，待推送任务接入）</n-descriptions-item>
      <n-descriptions-item label="日志目录">data/logs/（轮转，设上限）</n-descriptions-item>
      <n-descriptions-item label="当前状态">待实现</n-descriptions-item>
    </n-descriptions>
  </n-card>
</template>
