export const OFFICIAL_CUA_FRAME_INTEGRITY_META_KEY: string;
export const OFFICIAL_CUA_FRAME_MODEL_CONTENT_PROTECTION: string;
export const OFFICIAL_CUA_IMAGE_INLINE_BASE64_BYTES: number;

export declare function isOfficialCuaImageRefText(text: string): boolean;
export declare function containsOfficialCuaImageRefCredentialText(text: string): boolean;
export declare function containsImageRefAuthority(text: string): boolean;
export interface OfficialCuaImageRef {
  frame_id: string;
  credential: string;
  raster_sha256?: string;
  width?: number;
  height?: number;
  mimeType?: string;
}
export declare function parseOfficialCuaImageRef(text: string): OfficialCuaImageRef | undefined;
export declare function readRasterEnvelopeIdentity(input: unknown): string | undefined;
export declare function preserveOfficialCuaFrameResult<
  T extends { content?: unknown; isError?: boolean },
>(result: T, options?: unknown): Promise<T>;
export interface OfficialCuaFrameAttestation {
  kind: string;
}
export declare function attestOfficialCuaFrameContent(
  content: unknown,
  expectedKind?: string,
): OfficialCuaFrameAttestation | undefined;
export interface OfficialCuaFrameContentPair {
  image: any;
  imageRef: any;
  imageRefIndex: number;
  imageIndex: number;
}
export declare function findOfficialCuaFrameContentPair(
  content: unknown,
): OfficialCuaFrameContentPair | undefined;
export interface OfficialCuaImageRefTextOptions {
  frameId: string;
  rasterSha256: string;
  width: number;
  height: number;
  mimeType: string;
}
export declare function buildOfficialCuaImageRefText(options: OfficialCuaImageRefTextOptions): string;
export interface OfficialCuaFrameIntegrityMetaOptions {
  frameId: string;
  rasterSha256: string;
}
export declare function buildOfficialCuaFrameIntegrityMeta(
  options: OfficialCuaFrameIntegrityMetaOptions,
): { v: 1; frame_id: string; raster_sha256: string };
