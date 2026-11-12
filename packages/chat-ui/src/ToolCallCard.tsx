/** @jsxRuntime automatic */
import { ChevronDown, Globe2, Image as ImageIcon, LoaderCircle, RotateCcw, Search, Wrench, X } from "lucide-react";
import { useEffect, useId, useMemo, useRef, useState } from "react";
import type { ToolActivity } from "@axiom/chat-core/viewTypes";
import { isFetchTool, redactToolText, toolFailed, toolImageProgress, toolLabel, toolName, toolSearchSources } from "./toolPresentation";
import { ImagePreviewDialog } from "./ImagePreviewDialog";
import { decodeGeneratedPng } from "./generatedImage";
import { McpSourceLabel, mcpHost } from "./McpPrivacy";
import { useChatHost } from "./ChatHost";

const MAX_DETAIL_CHARS = 16_000;
const SENSITIVE_KEY = /authorization|api.?key|password|secret|token|cookie/i;

type ToolImage = {type:"image"; mimeType:"image/png"; data?:string; imageUrl?:string; assetId?:string; width?:number; height?:number};
function imageBlock(value: unknown): ToolImage | null {
  if (!value || typeof value !== "object") return null;
  const outer = value as Record<string,unknown>;
  const block = outer.type === "content" ? outer.content : outer;
  if (!block || typeof block !== "object") return null;
  const image = block as Record<string,unknown>;
  if (image.type !== "image" || image.mimeType !== "image/png") return null;
  if (typeof image.data === "string" && image.data.length <= 14_000_000) return image as ToolImage;
  const width = image.width, height = image.height;
  return ((typeof image.imageUrl === "string" && image.imageUrl.startsWith("blob:"))
    || (typeof image.assetId === "string" && image.assetId.length > 0 && image.assetId.length <= 128))
    && typeof width === "number" && typeof height === "number" && width >= 256 && height >= 256
    && width <= 8192 && height <= 8192 && width % 32 === 0 && height % 32 === 0 && width * height <= 1536 * 1536
    ? image as ToolImage : null;
}

function GeneratedToolImage({image, onRegenerate, regenerationDisabled}: {image:ToolImage; onRegenerate?:()=>Promise<unknown>; regenerationDisabled?:boolean}) {
  const {data, imageUrl, assetId, width, height} = image;
  const host = useChatHost();
  const figure = useRef<HTMLElement>(null);
  const [visible, setVisible] = useState(false);
  const [loadError, setLoadError] = useState("");
  const [loadAttempt, setLoadAttempt] = useState(0);
  const [downloadName] = useState(() => `${crypto.randomUUID()}.png`);
  const [preview, setPreview] = useState<string | null>(null);
  const [regenerating, setRegenerating] = useState(false);
  const [regenerationError, setRegenerationError] = useState("");
  const [loaded, setLoaded] = useState<{identity:string;url:string;width:number;height:number} | null>(null);
  useEffect(() => {
    if (!assetId || !figure.current) return;
    if (typeof IntersectionObserver === "undefined") { setVisible(true); return; }
    const observer = new IntersectionObserver(entries => {
      setVisible(entries.some(entry => entry.isIntersecting));
    }, { rootMargin: "160px" });
    observer.observe(figure.current);
    return () => observer.disconnect();
  }, [assetId]);
  useEffect(() => {
    const controller = new AbortController();
    let ownedUrl: string | null = null;
    let acquired = false;
    setLoaded(null); setLoadError("");
    if (assetId) {
      if (!visible) return;
      if (!host.loadGeneratedImage) { setLoadError("Image loading is unavailable. Restart Axiom."); return; }
      void host.loadGeneratedImage(assetId).then(source => {
        if (controller.signal.aborted) { host.releaseGeneratedImage?.(assetId); return; }
        acquired = true;
        setLoaded({ identity: assetId, url: source.imageUrl, width: source.width, height: source.height });
      }).catch(() => {
        if (!controller.signal.aborted) setLoadError("Couldn’t load this image.");
      });
    } else if (imageUrl && width && height) {
      // The document's preload owns and revokes this URL on account/runtime
      // change. Component unmount and StrictMode must not revoke shared assets.
      setLoaded({identity:imageUrl,url:imageUrl,width,height});
    } else if (data) {
      void decodeGeneratedPng(data, controller.signal).then(decoded => {
        if (!decoded || controller.signal.aborted) return;
        ownedUrl = URL.createObjectURL(decoded.blob);
        setLoaded({identity:data,url:ownedUrl,width:decoded.width,height:decoded.height});
      });
    }
    return () => {
      controller.abort();
      if (acquired && assetId) host.releaseGeneratedImage?.(assetId);
      if (ownedUrl) URL.revokeObjectURL(ownedUrl);
    };
  }, [data, imageUrl, assetId, width, height, visible, loadAttempt, host.loadGeneratedImage, host.releaseGeneratedImage]);
  const source = (!assetId || visible) && loaded?.identity === (assetId ?? imageUrl ?? data) ? loaded : null;
  return <figure ref={figure} className="my-3 w-full max-w-[32rem]">
    {source ? <button type="button" aria-label="View generated image" aria-haspopup="dialog" data-preserve-follow
      onClick={() => setPreview(source.url)}
      className="block w-full cursor-zoom-in overflow-hidden rounded-xl focus-visible:outline-2 focus-visible:outline-offset-2">
      <img src={source.url} alt="Generated image" width={source.width} height={source.height} decoding="async" loading="lazy" className="h-auto w-full" />
    </button> : <div className="flex w-full items-center justify-center overflow-hidden rounded-xl bg-[var(--wash-row)] text-[var(--color-text-tertiary)]"
      style={{ aspectRatio: `${width ?? 3} / ${height ?? 2}` }} aria-label="Generated image">
      {loadError ? <div role="alert" className="text-center text-[12px]">
        <p>{loadError}</p><button type="button" className="mt-2 underline" onClick={() => setLoadAttempt(value => value + 1)}>Retry loading image</button>
      </div> : visible || !assetId ? <span role="status" className="flex items-center gap-2 text-[12px]"><LoaderCircle size={16} className="animate-spin motion-reduce:animate-none" />Loading image…</span> : null}
    </div>}
    {onRegenerate ? <div className="mt-2 flex items-center justify-end gap-2">
      <button type="button" aria-label="Regenerate image" disabled={regenerating || regenerationDisabled}
        title="Generate a new image with the same prompt, references and size"
        onClick={() => {
          if (regenerating || regenerationDisabled) return;
          setRegenerating(true); setRegenerationError("");
          void onRegenerate().catch(() => setRegenerationError("Couldn’t regenerate this image. Check the result before trying again."))
            .finally(() => setRegenerating(false));
        }} className="inline-flex items-center gap-1.5 rounded-md px-2 py-1 text-[12px] text-[var(--color-text-secondary)] hover:bg-[var(--wash-chip-hover)] disabled:opacity-40">
        {regenerating ? <LoaderCircle size={13} className="animate-spin motion-reduce:animate-none" /> : <RotateCcw size={13} />}
        {regenerating ? "Regenerating…" : "Regenerate"}
      </button>
    </div> : null}
    {regenerationError ? <p role="alert" className="mt-1 text-[12px] text-[var(--color-danger-strong)]">{regenerationError}</p> : null}
    <figcaption className="mt-2 flex items-center justify-between text-[11px] text-[var(--color-text-tertiary)]">
      <span>AstreaAI</span>{source ? <a href={source.url} download={downloadName}
        onClick={event => { event.currentTarget.download = `${crypto.randomUUID()}.png`; }} className="hover:underline">Save image</a> : null}
    </figcaption>
    {source && preview === source.url ? <ImagePreviewDialog name="Generated image" src={source.url} onClose={() => setPreview(null)} /> : null}
  </figure>;
}

function safeDetail(value: unknown): string {
  const seen = new WeakSet<object>();
  const serialized = JSON.stringify(
    value,
    (key, nested) => {
      if (SENSITIVE_KEY.test(key)) return "[REDACTED]";
      if (nested && typeof nested === "object") {
        if (seen.has(nested)) return "[Circular]";
        seen.add(nested);
      }
      return nested;
    },
    2,
  ) ?? String(value);
  const text = redactToolText(serialized);
  return text.length > MAX_DETAIL_CHARS ? `${text.slice(0, MAX_DETAIL_CHARS)}\n… truncated` : text;
}

function toolIcon(tool: ToolActivity) {
  if (toolFailed(tool)) return <X size={14} className="text-[var(--color-danger)]" />;
  if (tool.status !== "completed") return <LoaderCircle size={14} className="animate-spin motion-reduce:animate-none" />;
  if (["generate", "generate_image", "edit_image"].includes(toolName(tool))) return <ImageIcon size={14} />;
  if (toolName(tool) === "fetch_url" || tool.kind === "fetch" || tool.kind === "web") return <Globe2 size={14} />;
  if (toolName(tool) === "web_search" || tool.kind === "search") return <Search size={14} />;
  return <Wrench size={14} />;
}

export function ToolCallCard({ tool, threadId, responseActive = false }: { tool: ToolActivity; threadId?: string; responseActive?: boolean }) {
  const host = useChatHost();
  const [open, setOpen] = useState(false);
  const detailsId = useId();
  const label = toolLabel(tool);
  const progress = toolImageProgress(tool);
  const sources = useMemo(() => toolSearchSources(tool), [tool]);
  const images = ["generate", "generate_image", "edit_image"].includes(toolName(tool)) && tool.status === "completed"
    ? tool.content.map(imageBlock).filter((image): image is NonNullable<typeof image> => image !== null) : [];
  const remoteHost = mcpHost(tool.mcpSource?.destination);
  const hasDetails = !isFetchTool(tool) && (!toolFailed(tool) || !!remoteHost)
    && (tool.input != null || tool.content.length > 0 || tool.locations.length > 0);
  const details = useMemo(
    () => open && hasDetails && !sources ? safeDetail({ input: tool.input, output: tool.content.map(c=>imageBlock(c) ? {type:"image",mimeType:"image/png"} : c), locations: tool.locations }) : "",
    [tool, open, hasDetails, sources],
  );
  const rowClass = "flex min-h-8 max-w-full items-center gap-2 rounded-md px-1.5 py-1 text-left leading-5 text-[var(--color-text-secondary)]";
  const row = <>
    <span className="shrink-0 text-[var(--color-text-tertiary)]" aria-hidden="true">{toolIcon(tool)}</span>
    <span className="min-w-0 truncate">{label}</span>
    {tool.mcpSource ? <McpSourceLabel source={tool.mcpSource} /> : null}
    {sources ? <span className="shrink-0 text-[11px] text-[var(--color-text-tertiary)]">{sources.length} {sources.length === 1 ? "source" : "sources"}</span> : null}
    {hasDetails ? <ChevronDown aria-hidden="true" size={12} className={`shrink-0 text-[var(--color-text-tertiary)] transition-transform ${open ? "rotate-0" : "-rotate-90"}`} /> : null}
  </>;

  return (
    <div className="min-w-0 text-[13px]" data-tool-call-id={tool.callId}>
      {hasDetails ? <button
        type="button"
        onClick={() => setOpen((value) => !value)}
        className={`${rowClass} transition-colors hover:bg-[var(--color-bg-surface-hover)]`}
        aria-expanded={open}
        aria-controls={detailsId}
        aria-label={`${label}; show details`}
        title={label}
      >
        {row}
      </button> : <div className={rowClass} title={label}>{row}</div>}
      {progress && tool.status !== "completed" ? <div role="progressbar" aria-label="Image generation progress" aria-valuemin={0} aria-valuemax={100}
        aria-valuenow={progress.percent} className="mx-1.5 mt-1 h-1 max-w-[32rem] overflow-hidden rounded-full bg-[var(--color-border)]">
        <div className="h-full rounded-full bg-[var(--color-text-secondary)] transition-[width] motion-reduce:transition-none" style={{width: `${progress.percent}%`}} />
      </div> : null}
      {images.map((image,index)=><GeneratedToolImage key={index} image={image}
        regenerationDisabled={responseActive}
        onRegenerate={threadId && host.regenerateImage ? () => host.regenerateImage!(threadId, tool.callId) : undefined} />)}
      {hasDetails && open ? (
        <div id={detailsId} className="selectable ml-3.5 mt-1 max-h-80 overflow-auto border-l border-[var(--color-border)] pl-4 pr-2 text-[12px] leading-5 text-[var(--color-text-secondary)]">
          {remoteHost ? <div className="mb-2 space-y-1 pt-1 text-[12px] leading-5">
            <p className="break-all"><strong>Destination:</strong> {remoteHost}</p>
            <p>{tool.status === "completed" ? "Inputs sent to this service." : tool.started === true ? "Request attempted. If interrupted, the service may have received these inputs or completed the action." : tool.started === false ? (tool.status === "pending" ? "Inputs will be sent if this call is approved." : "This call did not start; inputs were not sent by this call.") : "Transmission status is unavailable for this saved call."}</p>
            <p className="text-[var(--color-text-tertiary)]">Remote tool calls are outside Axiom’s attested environment.</p>
          </div> : null}
          {sources ? (
            sources.length ? <ul className="space-y-3 py-1">
              {sources.map((source, index) => <li key={`${index}:${source.url}`} className="min-w-0 break-words">
                <a href={source.url} target="_blank" rel="noopener noreferrer" className="font-medium text-[var(--color-text-primary)] hover:underline">{source.title}</a>
                <div className="truncate text-[11px] text-[var(--color-text-tertiary)]">{source.host}</div>
                {source.snippet ? <p>{source.snippet}</p> : null}
              </li>)}
            </ul> : <p className="py-1">No results found.</p>
          ) : <pre className="py-1 font-mono text-[11.5px] whitespace-pre-wrap break-words">{details}</pre>}
        </div>
      ) : null}
    </div>
  );
}
