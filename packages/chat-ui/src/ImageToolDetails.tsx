/** @jsxRuntime automatic */
import { LoaderCircle } from "lucide-react";
import { Fragment, useEffect, useState, type ReactNode } from "react";
import { useChatHost, type ChatHost } from "./ChatHost";
import { ImagePreviewDialog } from "./ImagePreviewDialog";
import type { ImageRequestSummary } from "./toolPresentation";

type ResolvedImage = { url: string; name: string; origin: string };

const MIME = /^image\/(?:png|jpeg|webp|gif)$/;
const ORIGIN_LABELS: Record<string, string> = { uploaded: "Uploaded image", generated: "Generated image", edited: "Edited image" };
// Resolved images are immutable for a thread-local ID, so expanding the same
// card again (or another card showing the same reference) reuses the bytes.
const MAX_CACHED_IMAGES = 12;
const cache = new Map<string, Promise<ResolvedImage>>();

function resolveCached(host: ChatHost, threadId: string, imageId: string): Promise<ResolvedImage> {
  const key = `${threadId}\0${imageId}`;
  const cached = cache.get(key);
  if (cached) {
    cache.delete(key);
    cache.set(key, cached);
    return cached;
  }
  const pending = host.resolveImage!(threadId, imageId).then((image) => {
    if (image.imageId !== imageId || !MIME.test(image.mimeType) || typeof image.data !== "string") {
      throw new Error("Conversation image identity differs");
    }
    return { url: `data:${image.mimeType};base64,${image.data}`, name: image.name, origin: image.origin };
  });
  cache.set(key, pending);
  pending.catch(() => { if (cache.get(key) === pending) cache.delete(key); });
  while (cache.size > MAX_CACHED_IMAGES) cache.delete(cache.keys().next().value!);
  return pending;
}

function useConversationImage(threadId: string | undefined, imageId: string) {
  const host = useChatHost();
  const [state, setState] = useState<{ image: ResolvedImage | null; error: string }>({ image: null, error: "" });
  const [attempt, setAttempt] = useState(0);
  useEffect(() => {
    let active = true;
    setState({ image: null, error: "" });
    if (!threadId || !host.resolveImage) {
      setState({ image: null, error: "Image previews are unavailable." });
      return;
    }
    void resolveCached(host, threadId, imageId).then(
      (image) => { if (active) setState({ image, error: "" }); },
      () => { if (active) setState({ image: null, error: "Couldn’t load this image." }); },
    );
    return () => { active = false; };
  }, [threadId, imageId, attempt, host.resolveImage]);
  return { ...state, retry: () => setAttempt((value) => value + 1) };
}

function ImageId({ id }: { id: string }) {
  return <code title={id} className="min-w-0 truncate rounded-md bg-[var(--wash-chip)] px-1.5 py-0.5 font-mono text-[11px] text-[var(--color-text-secondary)]">{id}</code>;
}

function ImageSlot({ image, error, retry, alt, label, className }: {
  image: ResolvedImage | null; error: string; retry: () => void; alt: string; label: string; className: string;
}) {
  const [preview, setPreview] = useState(false);
  if (image) return <>
    <button type="button" aria-label={label} aria-haspopup="dialog" data-preserve-follow onClick={() => setPreview(true)}
      className={`${className} block cursor-zoom-in overflow-hidden focus-visible:outline-2 focus-visible:outline-offset-2`}>
      <img src={image.url} alt={alt} decoding="async" draggable={false} className="h-full w-full object-contain" />
    </button>
    {preview ? <ImagePreviewDialog name={image.origin === "uploaded" && image.name ? image.name : alt} src={image.url}
      onClose={() => setPreview(false)} /> : null}
  </>;
  return <div className={`${className} flex items-center justify-center text-[var(--color-text-tertiary)]`}>
    {error ? <div role="alert" className="px-2 text-center text-[11.5px]">
      <p>{error}</p>
      <button type="button" className="mt-1 underline hover:text-[var(--color-text-secondary)]" onClick={retry}>Retry</button>
    </div> : <span role="status" aria-label="Loading image"><LoaderCircle size={16} className="animate-spin motion-reduce:animate-none" /></span>}
  </div>;
}

/** Expanded `view_image` details: the image the model inspected and its ID. */
export function ViewedImageDetails({ threadId, imageId }: { threadId?: string; imageId: string }) {
  const { image, error, retry } = useConversationImage(threadId, imageId);
  const origin = image ? ORIGIN_LABELS[image.origin] ?? "Image" : null;
  return <figure className="py-1.5" data-viewed-image={imageId}>
    <ImageSlot image={image} error={error} retry={retry} alt={`Image ${imageId}`} label={`View image ${imageId}`}
      className="h-56 w-full max-w-[22rem] rounded-xl bg-[var(--surface-track)]" />
    <figcaption className="mt-2 flex min-w-0 max-w-[22rem] flex-wrap items-center gap-x-2 gap-y-1">
      <ImageId id={imageId} />
      {origin ? <span className="min-w-0 truncate text-[11.5px] text-[var(--color-text-tertiary)]"
        title={image?.origin === "uploaded" && image.name ? `${origin} · ${image.name}` : origin}>
        {origin}{image?.origin === "uploaded" && image.name ? ` · ${image.name}` : ""}
      </span> : null}
    </figcaption>
  </figure>;
}

function ReferenceImageCard({ threadId, imageId, index }: { threadId?: string; imageId: string; index: number }) {
  const { image, error, retry } = useConversationImage(threadId, imageId);
  return <li data-reference-image={imageId}
    className="w-36 min-w-0 shrink-0 overflow-hidden rounded-[12px] border border-[var(--color-border)] bg-[var(--wash-chip)]">
    <ImageSlot image={image} error={error} retry={retry} alt={`Reference image ${index + 1}`}
      label={`View reference image ${index + 1}`} className="h-24 w-full bg-[var(--surface-track)]" />
    <div className="flex flex-col gap-1 px-2 py-1.5">
      <span className="text-[11.5px] font-medium text-[var(--color-text-primary)]">{`<image${index + 1}>`}</span>
      <ImageId id={imageId} />
    </div>
  </li>;
}

function Section({ label, children }: { label: string; children: ReactNode }) {
  return <section className="min-w-0">
    <h4 className="mb-1 text-[11.5px] font-medium text-[var(--color-text-tertiary)]">{label}</h4>
    {children}
  </section>;
}

/** Show `<imageN>` references in the prompt as the same labels the cards use. */
function PromptText({ text }: { text: string }) {
  const parts = text.split(/(<image[1-9]\d?>)/g);
  return <p className="max-h-60 overflow-y-auto whitespace-pre-wrap break-words text-[12.5px] leading-5 text-[var(--color-text-primary)]">
    {parts.map((part, index) => /^<image[1-9]\d?>$/.test(part)
      ? <span key={index} className="rounded bg-[var(--wash-chip)] px-1 py-px font-medium">{part}</span>
      : <Fragment key={index}>{part}</Fragment>)}
  </p>;
}

/** Expanded image-request details, shown above the resulting image. */
export function ImageRequestDetails({ threadId, request, outputSize }: {
  threadId?: string; request: ImageRequestSummary; outputSize?: { width: number; height: number } | null;
}) {
  const aspect = request.aspectRatio
    ?? (request.references.length ? "Same as <image1>" : "Default");
  return <div className="flex min-w-0 flex-col gap-3 py-1.5" data-image-request-details>
    <Section label="Prompt"><PromptText text={request.prompt} /></Section>
    <div className="flex flex-wrap gap-x-8 gap-y-3">
      <Section label="Aspect ratio"><p className="text-[12.5px] text-[var(--color-text-primary)]">{aspect}</p></Section>
      {outputSize ? <Section label="Size">
        <p className="text-[12.5px] tabular-nums text-[var(--color-text-primary)]">{outputSize.width} × {outputSize.height}</p>
      </Section> : null}
    </div>
    {request.references.length ? <Section label={request.references.length === 1 ? "Reference image" : "Reference images"}>
      <ul className="flex max-w-full flex-wrap gap-2">
        {request.references.map((id, index) => <ReferenceImageCard key={id} threadId={threadId} imageId={id} index={index} />)}
      </ul>
    </Section> : null}
  </div>;
}
