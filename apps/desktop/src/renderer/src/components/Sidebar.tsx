import {
  DndContext,
  DragOverlay,
  PointerSensor,
  closestCenter,
  useDraggable,
  useDroppable,
  useSensor,
  useSensors,
  type DragEndEvent,
  type DragStartEvent,
} from "@dnd-kit/core";
import {
  ChevronDown,
  Folder,
  FolderPlus,
  MoreHorizontal,
  PanelLeftClose,
  Plus,
  Search,
  Settings,
  Shield,
  Trash2,
} from "lucide-react";
import { useMemo, useState } from "react";
import { AxiomLogo } from "../brand/AxiomLogo";
import type { Folder as FolderType, Thread } from "../types";

interface SidebarProps {
  folders: FolderType[];
  threads: Thread[];
  activeThreadId: string | null;
  proxyActive: boolean;
  welcomeActive: boolean;
  collapsed: boolean;
  onCollapse: () => void;
  onNewChat: () => void;
  onOpenProxy: () => void;
  onSelectThread: (id: string) => void;
  onCreateFolder: () => void;
  onToggleFolder: (id: string) => void;
  onRenameFolder: (id: string, name: string) => void;
  onDeleteFolder: (id: string) => void;
  onDeleteThread: (id: string) => void;
  onMoveThread: (threadId: string, folderId: string | null) => void;
  onOpenSettings: () => void;
}

function ThreadRow({
  thread,
  active,
  nested,
  onSelect,
  onDelete,
}: {
  thread: Thread;
  active: boolean;
  nested?: boolean;
  onSelect: () => void;
  onDelete: () => void;
}) {
  const { attributes, listeners, setNodeRef, isDragging } = useDraggable({
    id: `thread:${thread.id}`,
    data: { type: "thread", threadId: thread.id },
  });
  const [menuOpen, setMenuOpen] = useState(false);

  return (
    <div
      ref={setNodeRef}
      className={[
        "group relative flex w-full items-center rounded-lg text-[13.5px] transition-colors",
        nested ? "pl-2" : "",
        active
          ? "bg-[var(--color-bg-surface-hover)] text-[var(--color-text-primary)]"
          : "text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-surface)] hover:text-[var(--color-text-primary)]",
        isDragging ? "opacity-40" : "",
      ].join(" ")}
    >
      <button
        type="button"
        onClick={onSelect}
        className="min-w-0 flex-1 truncate rounded-lg px-3 py-1.5 text-left"
        {...listeners}
        {...attributes}
      >
        {thread.title}
      </button>
      <div className="relative mr-1 shrink-0">
        <button
          type="button"
          onClick={(event) => {
            event.stopPropagation();
            setMenuOpen((open) => !open);
          }}
          className="flex h-6 w-6 items-center justify-center rounded-md text-[var(--color-text-tertiary)] opacity-0 transition-opacity hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-secondary)] group-hover:opacity-100"
          aria-label={`Open actions for ${thread.title}`}
        >
          <MoreHorizontal size={14} />
        </button>
        {menuOpen ? (
          <div className="animate-pop-in absolute right-0 top-7 z-40 w-36 origin-top-right rounded-lg border border-[var(--color-border)] bg-[var(--color-bg-surface)] py-1 shadow-xl">
            <button
              type="button"
              onClick={() => {
                setMenuOpen(false);
                onDelete();
              }}
              className="flex w-full items-center gap-2 px-2.5 py-1.5 text-left text-[12.5px] text-[var(--color-cherry-bright)] transition-colors hover:bg-[var(--color-bg-surface-hover)]"
            >
              <Trash2 size={13} />
              <span>Delete chat</span>
            </button>
          </div>
        ) : null}
      </div>
    </div>
  );
}

function FolderBlock({
  folder,
  threads,
  activeThreadId,
  onToggle,
  onSelectThread,
  onDeleteThread,
  onRename,
  onDeleteFolder,
}: {
  folder: FolderType;
  threads: Thread[];
  activeThreadId: string | null;
  onToggle: () => void;
  onSelectThread: (id: string) => void;
  onDeleteThread: (id: string) => void;
  onRename: (name: string) => void;
  onDeleteFolder: () => void;
}) {
  const { setNodeRef, isOver } = useDroppable({
    id: `folder:${folder.id}`,
    data: { type: "folder", folderId: folder.id },
  });
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(folder.name);

  return (
    <div
      ref={setNodeRef}
      className={[
        "rounded-lg",
        isOver ? "bg-[var(--color-cherry-glow)] ring-1 ring-[var(--color-border-accent)]" : "",
      ].join(" ")}
    >
      <div className="group flex items-center">
        <button
          type="button"
          onClick={onToggle}
          className="flex min-w-0 flex-1 items-center gap-2 rounded-lg px-2 py-1.5 text-left text-[12.5px] text-[var(--color-text-secondary)] transition-colors hover:bg-[var(--color-bg-surface)] hover:text-[var(--color-text-primary)]"
        >
          <ChevronDown
            size={14}
            className={`shrink-0 transition-transform ${folder.collapsed ? "-rotate-90" : ""}`}
          />
          <Folder size={14} className="shrink-0 text-[var(--color-text-tertiary)]" />
          {editing ? (
            <input
              autoFocus
              value={draft}
              onChange={(event) => setDraft(event.target.value)}
              onClick={(event) => event.stopPropagation()}
              onBlur={() => {
                onRename(draft.trim() || folder.name);
                setEditing(false);
              }}
              onKeyDown={(event) => {
                if (event.key === "Enter") {
                  onRename(draft.trim() || folder.name);
                  setEditing(false);
                }
              }}
              className="min-w-0 flex-1 bg-transparent text-[12.5px] text-[var(--color-text-primary)] outline-none"
            />
          ) : (
            <span
              className="min-w-0 flex-1 truncate"
              onDoubleClick={(event) => {
                event.stopPropagation();
                setDraft(folder.name);
                setEditing(true);
              }}
            >
              {folder.name}
            </span>
          )}
        </button>
        <button
          type="button"
          onClick={onDeleteFolder}
          className="mr-1 flex h-6 w-6 items-center justify-center rounded-md text-[var(--color-text-tertiary)] opacity-0 transition-opacity hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-cherry-bright)] group-hover:opacity-100"
          aria-label={`Delete folder ${folder.name}`}
        >
          <Trash2 size={12} />
        </button>
      </div>
      {!folder.collapsed ? (
        <div className="ml-3 border-l border-white/[0.04] pl-1 pb-1">
          {threads.length === 0 ? (
            <div className="px-3 py-1.5 text-[11.5px] text-[var(--color-text-muted)]">
              Drop chats here
            </div>
          ) : (
            threads.map((thread) => (
              <ThreadRow
                key={thread.id}
                thread={thread}
                nested
                active={thread.id === activeThreadId}
                onSelect={() => onSelectThread(thread.id)}
                onDelete={() => onDeleteThread(thread.id)}
              />
            ))
          )}
        </div>
      ) : null}
    </div>
  );
}

export function Sidebar({
  folders,
  threads,
  activeThreadId,
  proxyActive,
  welcomeActive,
  collapsed,
  onCollapse,
  onNewChat,
  onOpenProxy,
  onSelectThread,
  onCreateFolder,
  onToggleFolder,
  onRenameFolder,
  onDeleteFolder,
  onDeleteThread,
  onMoveThread,
  onOpenSettings,
}: SidebarProps) {
  const [query, setQuery] = useState("");
  const [activeDragId, setActiveDragId] = useState<string | null>(null);
  const sensors = useSensors(
    useSensor(PointerSensor, {
      activationConstraint: { distance: 8 },
    }),
  );

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return threads;
    return threads.filter((thread) => thread.title.toLowerCase().includes(q));
  }, [query, threads]);

  const loose = filtered.filter((thread) => thread.folderId === null);
  const draggingThread = threads.find((thread) => `thread:${thread.id}` === activeDragId);

  const onDragStart = (event: DragStartEvent) => {
    setActiveDragId(String(event.active.id));
  };

  const onDragEnd = (event: DragEndEvent) => {
    setActiveDragId(null);
    const overId = event.over?.id ? String(event.over.id) : null;
    const activeId = String(event.active.id);
    if (!overId || !activeId.startsWith("thread:")) return;
    const threadId = activeId.slice("thread:".length);
    if (overId === "folder:loose") {
      onMoveThread(threadId, null);
      return;
    }
    if (overId.startsWith("folder:")) {
      onMoveThread(threadId, overId.slice("folder:".length));
    }
  };

  if (collapsed) return null;

  return (
    <DndContext
      sensors={sensors}
      collisionDetection={closestCenter}
      onDragStart={onDragStart}
      onDragEnd={onDragEnd}
      onDragCancel={() => setActiveDragId(null)}
    >
      <SidebarFrame
        folders={folders}
        filtered={filtered}
        loose={loose}
        query={query}
        setQuery={setQuery}
        activeThreadId={activeThreadId}
        proxyActive={proxyActive}
        welcomeActive={welcomeActive}
        draggingThread={draggingThread}
        onCollapse={onCollapse}
        onNewChat={onNewChat}
        onOpenProxy={onOpenProxy}
        onSelectThread={onSelectThread}
        onCreateFolder={onCreateFolder}
        onToggleFolder={onToggleFolder}
        onRenameFolder={onRenameFolder}
        onDeleteFolder={onDeleteFolder}
        onDeleteThread={onDeleteThread}
        onOpenSettings={onOpenSettings}
      />
    </DndContext>
  );
}

function SidebarFrame({
  folders,
  filtered,
  loose,
  query,
  setQuery,
  activeThreadId,
  proxyActive,
  welcomeActive,
  draggingThread,
  onCollapse,
  onNewChat,
  onOpenProxy,
  onSelectThread,
  onCreateFolder,
  onToggleFolder,
  onRenameFolder,
  onDeleteFolder,
  onDeleteThread,
  onOpenSettings,
}: {
  folders: FolderType[];
  filtered: Thread[];
  loose: Thread[];
  query: string;
  setQuery: (value: string) => void;
  activeThreadId: string | null;
  proxyActive: boolean;
  welcomeActive: boolean;
  draggingThread: Thread | undefined;
  onCollapse: () => void;
  onNewChat: () => void;
  onOpenProxy: () => void;
  onSelectThread: (id: string) => void;
  onCreateFolder: () => void;
  onToggleFolder: (id: string) => void;
  onRenameFolder: (id: string, name: string) => void;
  onDeleteFolder: (id: string) => void;
  onDeleteThread: (id: string) => void;
  onOpenSettings: () => void;
}) {
  const { setNodeRef: setLooseRef, isOver: looseOver } = useDroppable({
    id: "folder:loose",
    data: { type: "loose" },
  });

  return (
    <>
      <aside className="flex h-full w-full shrink-0 flex-col bg-[var(--color-bg-sidebar)]">
        <div className="drag-region flex h-10 items-center justify-between px-3">
          <div className="no-drag flex items-center pl-1">
            <AxiomLogo className="h-[15px] w-auto text-[var(--color-text-primary)]" />
          </div>
          <button
            type="button"
            onClick={onCollapse}
            className="no-drag rounded-md p-1.5 text-[var(--color-text-tertiary)] transition-colors hover:bg-[var(--color-bg-surface-hover)] hover:text-[var(--color-text-secondary)]"
            aria-label="Collapse sidebar"
          >
            <PanelLeftClose size={16} />
          </button>
        </div>

        <nav className="no-drag px-2 pb-2 pt-1 flex flex-col gap-0.5">
          <button
            type="button"
            onClick={onNewChat}
            className={[
              "group flex w-full items-center gap-3 rounded-lg px-3 py-2 text-[13.5px] transition-[background-color,transform] duration-150 hover:bg-[var(--color-bg-surface-hover)] active:scale-[0.98]",
              welcomeActive
                ? "bg-[var(--color-bg-surface-hover)] text-[var(--color-text-primary)]"
                : "text-[var(--color-text-primary)]",
            ].join(" ")}
          >
            <span className="flex h-5 w-5 shrink-0 items-center justify-center rounded-full bg-[var(--color-cherry)] transition-transform duration-[250ms] ease-[var(--ease-spring)] group-hover:scale-110">
              <Plus size={13} strokeWidth={3} className="text-[#140709]" />
            </span>
            <span>New chat</span>
          </button>
          <button
            type="button"
            onClick={onOpenProxy}
            className={[
              "flex w-full items-center gap-3 rounded-lg px-3 py-2 text-[13.5px] transition-colors",
              proxyActive
                ? "bg-[var(--color-bg-surface-hover)] text-[var(--color-text-primary)]"
                : "text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-surface)] hover:text-[var(--color-text-primary)]",
            ].join(" ")}
          >
            <span className="flex h-5 w-5 shrink-0 items-center justify-center">
              <Shield size={16} className={proxyActive ? "text-[var(--color-cherry)]" : ""} />
            </span>
            <span>Proxy</span>
          </button>

          <div className="flex items-center gap-1 px-1 pt-1">
            <div className="flex min-w-0 flex-1 items-center gap-2 rounded-lg border border-transparent bg-[var(--color-bg-surface)] px-2.5 py-1.5 focus-within:border-[var(--color-border-accent)]">
              <Search size={14} className="text-[var(--color-text-tertiary)]" />
              <input
                value={query}
                onChange={(event) => setQuery(event.target.value)}
                placeholder="Search chats"
                className="w-full bg-transparent text-[13px] text-[var(--color-text-primary)] outline-none placeholder:text-[var(--color-text-tertiary)]"
              />
            </div>
            <button
              type="button"
              onClick={onCreateFolder}
              className="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg text-[var(--color-text-tertiary)] transition-colors hover:bg-[var(--color-bg-surface-hover)] hover:text-[var(--color-text-primary)]"
              title="New folder"
              aria-label="New folder"
            >
              <FolderPlus size={15} />
            </button>
          </div>
        </nav>

        <div className="no-drag min-h-0 flex-1 overflow-y-auto px-2 pb-2">
          {folders.map((folder) => (
            <FolderBlock
              key={folder.id}
              folder={folder}
              threads={filtered.filter((thread) => thread.folderId === folder.id)}
              activeThreadId={activeThreadId}
              onToggle={() => onToggleFolder(folder.id)}
              onSelectThread={onSelectThread}
              onDeleteThread={onDeleteThread}
              onRename={(name) => onRenameFolder(folder.id, name)}
              onDeleteFolder={() => onDeleteFolder(folder.id)}
            />
          ))}

          <div
            ref={setLooseRef}
            className={[
              "rounded-lg",
              looseOver ? "bg-[var(--color-cherry-glow)]" : "",
            ].join(" ")}
          >
            {loose.map((thread) => (
              <ThreadRow
                key={thread.id}
                thread={thread}
                active={thread.id === activeThreadId && !proxyActive}
                onSelect={() => onSelectThread(thread.id)}
                onDelete={() => onDeleteThread(thread.id)}
              />
            ))}
            {filtered.length === 0 && query ? (
              <div className="px-3 pt-6 text-[12.5px] text-[var(--color-text-tertiary)]">
                No chats match “{query}”.
              </div>
            ) : null}
          </div>
        </div>

        <div className="no-drag border-t border-white/[0.045] px-2 py-2">
          <button
            type="button"
            onClick={onOpenSettings}
            className="flex w-full items-center gap-2.5 rounded-lg px-2 py-1.5 transition-colors hover:bg-[var(--color-bg-surface-hover)]"
          >
            <div className="flex h-7 w-7 items-center justify-center rounded-full bg-gradient-to-br from-[var(--color-cherry)] to-[var(--color-cherry-deep)] text-[11px] font-semibold text-[#140709]">
              SA
            </div>
            <div className="flex min-w-0 flex-1 flex-col items-start leading-tight">
              <span className="max-w-full truncate text-[12.5px] text-[var(--color-text-primary)]">
                salt
              </span>
              <span className="text-[11px] text-[var(--color-text-tertiary)]">Private account</span>
            </div>
            <Settings size={14} className="text-[var(--color-text-tertiary)]" />
          </button>
        </div>
      </aside>

      <DragOverlay>
        {draggingThread ? (
          <div className="rounded-lg bg-[var(--color-bg-elevated)] px-3 py-1.5 text-[13px] text-[var(--color-text-primary)] shadow-xl">
            {draggingThread.title}
          </div>
        ) : null}
      </DragOverlay>
    </>
  );
}
