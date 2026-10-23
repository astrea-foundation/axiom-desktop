/** Only formats transported as image bytes by an enabled provider adapter. */
export const IMAGE_ATTACHMENT_ACCEPT = ".png,.jpg,.jpeg,.webp,.gif,image/png,image/jpeg,image/webp,image/gif";

export interface AttachmentAffordance {
  blocked: boolean;
  hint: string;
  accept: string;
}

export function attachmentAffordance(
  model: { shortLabel: string; providerLabel: string; supportsImages?: boolean } | undefined,
  disabledReason?: string,
): AttachmentAffordance {
  const accept = IMAGE_ATTACHMENT_ACCEPT;
  if (disabledReason) return { blocked: true, hint: disabledReason, accept };
  if (!model) return { blocked: true, hint: "Attachments aren’t available until a supported model is selected.", accept };
  if (!model.supportsImages) return { blocked: true, hint: `${model.providerLabel}’s ${model.shortLabel} doesn’t support attachments.`, accept };
  return { blocked: false, hint: "Attach images (PNG, JPEG, WebP, or GIF)", accept };
}
