export function openGitHubAuthorization(
  popup: Window | null,
  verificationUri: string,
  userCode: string,
): void {
  if (popup) {
    popup.opener = null;
    popup.location.href = verificationUri;
  } else {
    window.open(verificationUri, "_blank", "noopener,noreferrer");
  }
  void navigator.clipboard?.writeText(userCode).catch(() => undefined);
}
