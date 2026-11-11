import { createEncryptedCredentialStore, type WebCredentialStore } from "@mode/remote-core/web";

const DB_NAME = "mode-remote-v3";
const DB_VERSION = 1;
const STORE_NAME = "kv";
const LOCAL_PREFIX = "mode.remote.v3.";

/**
 * 浏览器默认凭据存储：优先 IndexedDB，隐私模式不可用时回落 localStorage。
 *
 * IndexedDB 分支额外挂 AES-GCM 信封：私钥/token 不再以明文落盘，密钥是不可导出的
 * CryptoKey（同样存 IndexedDB）。localStorage 存不了 CryptoKey，保持明文并在文档中声明。
 */
export function createWebCredentialStore(): WebCredentialStore {
  if (typeof indexedDB !== "undefined") {
    return createEncryptedCredentialStore(createIndexedDbStore());
  }
  return createLocalStorageStore();
}

function openDatabase(): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    const request = indexedDB.open(DB_NAME, DB_VERSION);
    request.onupgradeneeded = () => {
      const db = request.result;
      if (!db.objectStoreNames.contains(STORE_NAME)) {
        db.createObjectStore(STORE_NAME);
      }
    };
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error ?? new Error("IndexedDB 打开失败"));
  });
}

async function withStore<T>(
  mode: IDBTransactionMode,
  run: (store: IDBObjectStore) => Promise<T>,
): Promise<T> {
  const db = await openDatabase();
  return await new Promise<T>((resolve, reject) => {
    const tx = db.transaction(STORE_NAME, mode);
    const store = tx.objectStore(STORE_NAME);
    let result: T;
    void run(store).then(
      (value) => {
        result = value;
      },
      (error) => {
        db.close();
        reject(error);
      },
    );
    tx.oncomplete = () => {
      db.close();
      resolve(result);
    };
    tx.onerror = () => {
      db.close();
      reject(tx.error ?? new Error("IndexedDB 事务失败"));
    };
  });
}

function request<T>(store: IDBObjectStore, method: "get", key: string): Promise<T | undefined> {
  return new Promise((resolve, reject) => {
    const result = store.get(key);
    result.onsuccess = () => resolve(result.result as T | undefined);
    result.onerror = () => reject(result.error ?? new Error("IndexedDB 读取失败"));
  });
}

async function putValue(store: IDBObjectStore, key: string, value: unknown): Promise<void> {
  await new Promise<void>((resolve, reject) => {
    const result = store.put(value, key);
    result.onsuccess = () => resolve();
    result.onerror = () => reject(result.error ?? new Error("IndexedDB 写入失败"));
  });
}

function createIndexedDbStore(): WebCredentialStore {
  return {
    get: async (key) => (await withStore("readonly", (store) => request<string>(store, "get", key))) ?? null,
    set: async (key, value) => {
      await withStore("readwrite", async (store) => {
        await putValue(store, key, value);
        return undefined as void;
      });
    },
    // 结构化对象：IndexedDB 可直接持久化 CryptoKey（localStorage 不具备）。
    getValue: async <T>(key: string) =>
      (await withStore("readonly", (store) => request<T>(store, "get", key))) ?? null,
    setValue: async <T>(key: string, value: T) => {
      await withStore("readwrite", async (store) => {
        await putValue(store, key, value);
        return undefined as void;
      });
    },
    delete: async (key) => {
      await withStore("readwrite", async (store) => {
        await new Promise<void>((resolve, reject) => {
          const result = store.delete(key);
          result.onsuccess = () => resolve();
          result.onerror = () => reject(result.error ?? new Error("IndexedDB 删除失败"));
        });
        return undefined as void;
      });
    },
  };
}

function createLocalStorageStore(): WebCredentialStore {
  return {
    get: async (key) => localStorage.getItem(`${LOCAL_PREFIX}${key}`),
    set: async (key, value) => {
      localStorage.setItem(`${LOCAL_PREFIX}${key}`, value);
    },
    delete: async (key) => {
      localStorage.removeItem(`${LOCAL_PREFIX}${key}`);
    },
  };
}
