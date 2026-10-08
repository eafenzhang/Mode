export interface HelloMessage {
  type: "mode-hello";
  version: string;
  platform: string;
  arch: string;
  pid: number;
}

export interface HelloAckMessage {
  type: "mode-hello-ack";
  version: string;
  clientId: string;
}
