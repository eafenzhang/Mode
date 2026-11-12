import { buildGithubDeviceAuthorizationUrl } from "@mode/shared";

export function openGitHubAuthorization(
  popup: Window | null,
  verificationUri: string,
  userCode: string,
): void {
  const url = buildGithubDeviceAuthorizationUrl(verificationUri, userCode);
  if (popup) {
    popup.opener = null;
    popup.location.href = url;
  } else {
    window.open(url, "_blank", "noopener,noreferrer");
  }
  void navigator.clipboard?.writeText(userCode).catch(() => undefined);
}
