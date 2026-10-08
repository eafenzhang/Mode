import {
  ChannelClient,
  MessagePortProtocol,
  ProxyChannel,
  type MessagePortLike,
  type MessagePortPayload,
} from "@mode/rpc";
import {
  IModeTaskService,
  type IModeTaskService as IModeTaskServiceShape,
} from "#src/session/modeTaskService.js";
import {
  IModeAgentService,
  type IModeAgentService as IModeAgentServiceShape,
} from "#src/mode-agent/modeAgent.js";
import {
  IModeSessionService,
  type IModeSessionService as IModeSessionServiceShape,
} from "#src/mode-session/modeSession.js";
import {
  IModelSelectionService,
  type IModelSelectionService as IModelSelectionServiceShape,
} from "#src/model-provider/providerFacadeServices.js";

interface PortLike {
  on?(event: "message", listener: (event: { data: MessagePortPayload }) => void): void;
  off?(event: "message", listener: (event: { data: MessagePortPayload }) => void): void;
  addEventListener?(
    event: "message",
    listener: (event: { data: MessagePortPayload }) => void,
  ): void;
  removeEventListener?(
    event: "message",
    listener: (event: { data: MessagePortPayload }) => void,
  ): void;
  postMessage(message: MessagePortPayload): void;
  start?(): void;
  close?(): void;
}

function toMessagePortLike(port: PortLike): MessagePortLike {
  return {
    addEventListener(type, listener) {
      if (port.addEventListener) {
        port.addEventListener(type, listener);
        return;
      }
      port.on?.(type, listener);
    },
    removeEventListener(type, listener) {
      if (port.removeEventListener) {
        port.removeEventListener(type, listener);
        return;
      }
      port.off?.(type, listener);
    },
    postMessage(data) {
      port.postMessage(data);
    },
    start() {
      port.start?.();
    },
    close() {
      port.close?.();
    },
  };
}

export interface RemoteBotWorkspaceRuntimeServices {
  modeAgentService: IModeAgentServiceShape;
  modeTaskService: IModeTaskServiceShape;
  modeSessionService: IModeSessionServiceShape;
  modelSelectionService: IModelSelectionServiceShape;
}

export function createRemoteRuntimeServicesFromPort(
  port: unknown,
): RemoteBotWorkspaceRuntimeServices {
  const protocol = new MessagePortProtocol(toMessagePortLike(port as PortLike));
  const client = new ChannelClient(protocol);
  return {
    modeAgentService: ProxyChannel.toService<IModeAgentServiceShape>(
      client.getChannel(IModeAgentService.channelName),
    ),
    modeTaskService: ProxyChannel.toService<IModeTaskServiceShape>(
      client.getChannel(IModeTaskService.channelName),
    ),
    modeSessionService: ProxyChannel.toService<IModeSessionServiceShape>(
      client.getChannel(IModeSessionService.channelName),
    ),
    modelSelectionService: ProxyChannel.toService<IModelSelectionServiceShape>(
      client.getChannel(IModelSelectionService.channelName),
    ),
  };
}
