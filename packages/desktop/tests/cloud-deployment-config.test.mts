/**
 * 部署配置文件读取：安装时写一次，之后不再逐机设置环境变量。
 */
import assert from "node:assert/strict";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import {
  loadCloudDeploymentConfig,
  resetCloudDeploymentConfigCacheForTests,
} from "../src/host/cloudDeploymentConfig.js";

test("从 MODE_CLOUD_CONFIG_FILE 读取部署配置", () => {
  const dir = mkdtempSync(join(tmpdir(), "mode-cloud-config-"));
  try {
    const file = join(dir, "cloud-deployment.json");
    writeFileSync(
      file,
      JSON.stringify({
        cloudBaseUrl: "https://cloud.example.test",
        enrollToken: "enroll-token",
        relayToken: "relay-token",
      }),
    );
    resetCloudDeploymentConfigCacheForTests();
    const config = loadCloudDeploymentConfig(
      { MODE_CLOUD_CONFIG_FILE: file } as NodeJS.ProcessEnv,
      dir,
    );
    assert.deepEqual(config, {
      cloudBaseUrl: "https://cloud.example.test",
      enrollToken: "enroll-token",
      relayToken: "relay-token",
    });
  } finally {
    rmSync(dir, { recursive: true, force: true });
    resetCloudDeploymentConfigCacheForTests();
  }
});

test("忽略非法字段，只返回有效字符串", () => {
  const dir = mkdtempSync(join(tmpdir(), "mode-cloud-config-"));
  try {
    const file = join(dir, "cloud-deployment.json");
    writeFileSync(
      file,
      JSON.stringify({ cloudBaseUrl: "https://cloud.example.test", enrollToken: 42 }),
    );
    resetCloudDeploymentConfigCacheForTests();
    const config = loadCloudDeploymentConfig(
      { MODE_CLOUD_CONFIG_FILE: file } as NodeJS.ProcessEnv,
      dir,
    );
    assert.deepEqual(config, { cloudBaseUrl: "https://cloud.example.test" });
  } finally {
    rmSync(dir, { recursive: true, force: true });
    resetCloudDeploymentConfigCacheForTests();
  }
});
