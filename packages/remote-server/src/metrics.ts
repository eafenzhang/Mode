/** 远程控制 v3 的运行计数；只暴露数值，不包含设备 ID、token 或业务内容。 */
export class RemoteMetrics {
  readonly startedAt = Date.now();
  private controlConnections = 0;
  private connections = 0;
  private relayFrames = 0;
  private relayBytes = 0;
  private apiRequests = 0;

  controlOpened(): void {
    this.controlConnections += 1;
  }

  controlClosed(): void {
    this.controlConnections = Math.max(0, this.controlConnections - 1);
  }

  connectionOpened(): void {
    this.connections += 1;
  }

  connectionClosed(): void {
    this.connections = Math.max(0, this.connections - 1);
  }

  relayForwarded(bytes: number): void {
    this.relayFrames += 1;
    this.relayBytes += bytes;
  }

  apiRequested(): void {
    this.apiRequests += 1;
  }

  render(): string {
    const uptime = Math.floor((Date.now() - this.startedAt) / 1000);
    return [
      "# HELP mode_remote_uptime_seconds Process uptime in seconds.",
      "# TYPE mode_remote_uptime_seconds gauge",
      `mode_remote_uptime_seconds ${uptime}`,
      "# HELP mode_remote_control_connections Active Control WebSocket connections.",
      "# TYPE mode_remote_control_connections gauge",
      `mode_remote_control_connections ${this.controlConnections}`,
      "# HELP mode_remote_connections Active logical connections.",
      "# TYPE mode_remote_connections gauge",
      `mode_remote_connections ${this.connections}`,
      "# HELP mode_remote_relay_frames_total Relay frames forwarded.",
      "# TYPE mode_remote_relay_frames_total counter",
      `mode_remote_relay_frames_total ${this.relayFrames}`,
      "# HELP mode_remote_relay_bytes_total Ciphertext bytes forwarded over Relay.",
      "# TYPE mode_remote_relay_bytes_total counter",
      `mode_remote_relay_bytes_total ${this.relayBytes}`,
      "# HELP mode_remote_api_requests_total REST API requests.",
      "# TYPE mode_remote_api_requests_total counter",
      `mode_remote_api_requests_total ${this.apiRequests}`,
      "",
    ].join("\n");
  }
}
