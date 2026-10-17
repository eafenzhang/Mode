// Minimal Chrome extension API declarations used by the popup.
// Only the surface this extension needs; no @types/chrome dependency.
declare namespace chrome {
  namespace cookies {
    interface Cookie {
      name: string;
      value: string;
      domain: string;
      path: string;
      secure: boolean;
      httpOnly: boolean;
      sameSite?: string;
      expirationDate?: number;
      hostOnly?: boolean;
    }

    function getAll(details: {
      url?: string;
      name?: string;
      domain?: string;
      path?: string;
      secure?: boolean;
      session?: boolean;
    }): Promise<Cookie[]>;
  }

  namespace runtime {
    interface LastError {
      message?: string;
    }

    const lastError: LastError | undefined;

    function sendNativeMessage(application: string, message: unknown): Promise<unknown>;
  }
}
