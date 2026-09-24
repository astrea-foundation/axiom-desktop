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
  PanelLeftClose,
  Plus,
  Search,
  Shield,
} from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";
import { contextMenuTrigger, SidebarContextMenu, type SidebarMenuTarget } from "./SidebarContextMenu";
import { AccountMenu, type AccountMenuProps } from "./AccountMenu";
import { OverflowTitle } from "./OverflowTitle";
import { isMac, MAC_TRAFFIC_LIGHT_INSET_CLASS, useFullScreen } from "../lib/window-chrome";
import type { AccountPresentation } from "../signInFlow";
import type { Folder as FolderType, Thread } from "../types";

interface SidebarProps {
  folders: FolderType[];
  threads: Thread[];
  activeThreadId: string | null;
  proxyActive: boolean;
  welcomeActive: boolean;
  onCollapse: () => void;
  onNewChat: () => void;
  onOpenProxy: () => void;
  onSelectThread: (id: string) => void;
  onCreateFolder: () => void;
  onToggleFolder: (id: string) => void;
  onRenameFolder: (id: string) => void;
  onRenameThread: (id: string) => void;
  onDeleteFolder: (id: string) => void;
  onDeleteThread: (id: string) => void;
  onMoveThread: (threadId: string, folderId: string | null) => void;
  account: AccountPresentation;
  onSignIn: () => void;
  onLogout: AccountMenuProps["onLogout"];
  onOpenSettings: AccountMenuProps["onOpenSettings"];
  onOpenBalance: AccountMenuProps["onOpenBalance"];
  onRefreshBalance: AccountMenuProps["onRefreshBalance"];
  billing: AccountMenuProps["billing"];
  accountId: string | null;
  connected: boolean;
}

function ThreadRow({
  thread,
  active,
  nested,
  onSelect,
  onOpenMenu,
}: {
  thread: Thread;
  active: boolean;
  nested?: boolean;
  onSelect: () => void;
  onOpenMenu: (target: SidebarMenuTarget) => void;
}) {
  const { attributes, listeners, setNodeRef, isDragging } = useDraggable({
    id: `thread:${thread.id}`,
    data: { type: "thread", threadId: thread.id },
  });

  return (
    <div
      ref={setNodeRef}
      className={[
        "group relative flex h-8 w-full items-center rounded-[9px] text-[13px] transition-colors",
        nested ? "pl-2" : "",
        active
          ? "bg-[var(--surface-tile)] text-[var(--color-text-primary)] shadow-glass-tile"
          : "text-[var(--color-text-secondary)] hover:bg-[var(--wash-hover)] hover:text-[var(--color-text-primary)]",
        isDragging ? "opacity-40" : "",
      ].join(" ")}
    >
      <button
        type="button"
        onClick={onSelect}
        className="title-scroll-trigger flex h-full min-w-0 flex-1 items-center gap-2 rounded-[9px] px-2 text-left"
        {...listeners}
        {...attributes}
        data-thread-id={thread.id}
        aria-label={`${thread.status === "working" ? "Replying" : thread.status === "unread" ? "Unread reply" : "Viewed"} ${thread.title}`}
        {...contextMenuTrigger("thread", thread.id, onOpenMenu)}
      >
        <OverflowTitle text={thread.title} className="flex-1" />
        <span className="flex h-[6px] w-[6px] shrink-0" data-thread-indicator aria-hidden="true">
          {thread.status !== "idle" ? <i className={`thread-dot${thread.status === "working" ? " thread-dot-working" : ""}`} /> : null}
        </span>
      </button>
    </div>
  );
}

function FolderBlock({
  folder,
  threads,
  activeThreadId,
  onToggle,
  onSelectThread,
  onOpenMenu,
}: {
  folder: FolderType;
  threads: Thread[];
  activeThreadId: string | null;
  onToggle: () => void;
  onSelectThread: (id: string) => void;
  onOpenMenu: (target: SidebarMenuTarget) => void;
}) {
  const { setNodeRef, isOver } = useDroppable({
    id: `folder:${folder.id}`,
    data: { type: "folder", folderId: folder.id },
  });

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
          data-folder-id={folder.id}
          {...contextMenuTrigger("folder", folder.id, onOpenMenu)}
          className="title-scroll-trigger flex min-w-0 flex-1 items-center gap-2 rounded-lg px-2 py-1.5 text-left text-[12.5px] text-[var(--color-text-secondary)] transition-colors hover:bg-[var(--wash-hover)] hover:text-[var(--color-text-primary)]"
        >
          <ChevronDown
            size={14}
            className={`shrink-0 transition-transform ${folder.collapsed ? "-rotate-90" : ""}`}
          />
          <Folder size={14} className="shrink-0 text-[var(--color-text-tertiary)]" />
          <OverflowTitle text={folder.name} className="flex-1" />
        </button>
      </div>
      {!folder.collapsed ? (
        <div className="ml-3 pl-1 pb-1">
          {threads.length === 0 ? (
            <div className="px-3 py-1.5 text-[11.5px] text-[var(--color-text-muted)]">
              Drop threads here
            </div>
          ) : (
            threads.map((thread) => (
              <ThreadRow
                key={thread.id}
                thread={thread}
                nested
                active={thread.id === activeThreadId}
                onSelect={() => onSelectThread(thread.id)}
                onOpenMenu={onOpenMenu}
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
  onCollapse,
  onNewChat,
  onOpenProxy,
  onSelectThread,
  onCreateFolder,
  onToggleFolder,
  onRenameFolder,
  onRenameThread,
  onDeleteFolder,
  onDeleteThread,
  onMoveThread,
  account,
  onSignIn,
  onLogout,
  onOpenSettings,
  onOpenBalance,
  onRefreshBalance,
  billing,
  accountId,
  connected,
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
        onRenameThread={onRenameThread}
        onDeleteFolder={onDeleteFolder}
        onDeleteThread={onDeleteThread}
        account={account}
        onSignIn={onSignIn}
        onLogout={onLogout}
        onOpenSettings={onOpenSettings}
        onOpenBalance={onOpenBalance}
        onRefreshBalance={onRefreshBalance}
        billing={billing}
        accountId={accountId}
        connected={connected}
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
  onRenameThread,
  onDeleteFolder,
  onDeleteThread,
  account,
  onSignIn,
  onLogout,
  onOpenSettings,
  onOpenBalance,
  onRefreshBalance,
  billing,
  accountId,
  connected,
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
  onRenameFolder: (id: string) => void;
  onRenameThread: (id: string) => void;
  onDeleteFolder: (id: string) => void;
  onDeleteThread: (id: string) => void;
  account: AccountPresentation;
  onSignIn: () => void;
  onLogout: AccountMenuProps["onLogout"];
  onOpenSettings: AccountMenuProps["onOpenSettings"];
  onOpenBalance: AccountMenuProps["onOpenBalance"];
  onRefreshBalance: AccountMenuProps["onRefreshBalance"];
  billing: AccountMenuProps["billing"];
  accountId: string | null;
  connected: boolean;
}) {
  const { setNodeRef: setLooseRef, isOver: looseOver } = useDroppable({
    id: "folder:loose",
    data: { type: "loose" },
  });
  const fullScreen = useFullScreen();
  const trafficLightInset = isMac && !fullScreen;
  const [menu, setMenu] = useState<SidebarMenuTarget | null>(null);
  const closeMenu = useCallback(() => setMenu(null), []);
  // A menu must never outlive its row (search, deletion, collapse, or account switch).
  useEffect(() => {
    if (menu && (!menu.trigger.isConnected || menu.trigger.closest("[inert]"))) closeMenu();
  });
  useEffect(closeMenu, [account.kind, account.title, account.detail, query, closeMenu]);

  return (
    <>
      <aside className="relative flex h-full w-full shrink-0 flex-col bg-transparent">
        {/* macOS reserves a dedicated chrome row beside the traffic lights. */}
        {isMac ? (
          <div
            className={[
              "drag-region flex h-10 shrink-0 items-center justify-end pr-2",
              trafficLightInset ? MAC_TRAFFIC_LIGHT_INSET_CLASS : "pl-3",
            ].join(" ")}
          >
            <button
              type="button"
              onClick={onCollapse}
              className="no-drag rounded-md p-1.5 text-[var(--color-text-tertiary)] transition-colors hover:bg-[var(--color-bg-surface-hover)] hover:text-[var(--color-text-secondary)]"
              aria-label="Collapse sidebar"
            >
              <PanelLeftClose size={16} />
            </button>
          </div>
        ) : null}

        <nav className={`no-drag flex flex-col gap-2.5 px-3 pb-2 ${isMac ? "pt-1" : "pt-1.5"}`}>
          <div className="flex flex-col gap-0.5">
          <div className="flex items-center gap-1">
            <button
              type="button"
              onClick={onNewChat}
              aria-current={welcomeActive ? "page" : undefined}
              className="flex h-8 min-w-0 flex-1 items-center gap-2 rounded-[9px] pl-0 text-[13px] text-[var(--color-text-secondary)] transition-[background-color,color,padding-left] duration-200 ease-[var(--ease-out-expo)] hover:bg-[var(--wash-hover)] hover:pl-2 hover:text-[var(--color-text-primary)]"
            >
              <span className="flex h-[15px] w-[15px] shrink-0 items-center justify-center rounded-full border border-current">
                <Plus size={9} strokeWidth={2.6} />
              </span>
              <span>New thread</span>
            </button>
            {!isMac ? (
              <button
                type="button"
                onClick={onCollapse}
                className="shrink-0 rounded-md p-1.5 text-[var(--color-text-tertiary)] transition-colors hover:bg-[var(--color-bg-surface-hover)] hover:text-[var(--color-text-secondary)]"
                aria-label="Collapse sidebar"
              >
                <PanelLeftClose size={16} />
              </button>
            ) : null}
          </div>
          <button
            type="button"
            onClick={onOpenProxy}
            className={[
              "flex h-8 w-full items-center gap-2 rounded-[9px] text-[13px] transition-[background-color,color,padding-left] duration-200 ease-[var(--ease-out-expo)]",
              proxyActive
                ? "bg-[var(--surface-tile)] pl-2 text-[var(--color-text-primary)] shadow-glass-tile"
                : "pl-0 text-[var(--color-text-secondary)] hover:bg-[var(--wash-hover)] hover:pl-2 hover:text-[var(--color-text-primary)]",
            ].join(" ")}
          >
            <span className="flex h-[15px] w-[15px] shrink-0 items-center justify-center">
              <Shield size={14} />
            </span>
            <span>Proxy</span>
          </button>
          </div>

          <div className="flex items-center gap-1">
            <div className="flex min-w-0 flex-1 items-center gap-2 rounded-[8px] border border-[var(--color-border)] bg-[var(--color-bg-input)] px-2.5 py-1.5 transition-[border-color,box-shadow] hover:border-[var(--border-hover)] focus-within:border-[var(--color-border-accent)] focus-within:shadow-[0_0_0_3px_var(--ring-focus-halo)]">
              <Search size={14} className="text-[var(--color-text-tertiary)]" />
              <input
                value={query}
                onChange={(event) => setQuery(event.target.value)}
                placeholder="Search threads"
                className="w-full bg-transparent text-[13px] text-[var(--color-text-primary)] outline-none placeholder:text-[var(--color-text-muted)]"
              />
            </div>
            <button
              type="button"
              onClick={onCreateFolder}
              className="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg text-[var(--color-text-tertiary)] transition-colors hover:bg-[var(--wash-hover)] hover:text-[var(--color-text-primary)]"
              title="New folder"
              aria-label="New folder"
            >
              <FolderPlus size={15} />
            </button>
          </div>
        </nav>

        <div className="no-drag min-h-0 flex-1 overflow-y-auto px-3 pb-2 pt-3">
          <div className="ax-label px-2 pb-2">Threads</div>
          {folders.map((folder) => (
            <FolderBlock
              key={folder.id}
              folder={folder}
              threads={filtered.filter((thread) => thread.folderId === folder.id)}
              activeThreadId={activeThreadId}
              onToggle={() => onToggleFolder(folder.id)}
              onSelectThread={onSelectThread}
              onOpenMenu={setMenu}
            />
          ))}

          <div
            ref={setLooseRef}
            className={[
              "flex flex-col gap-0.5 rounded-lg",
              looseOver ? "bg-[var(--color-cherry-glow)]" : "",
            ].join(" ")}
          >
            {loose.map((thread) => (
              <ThreadRow
                key={thread.id}
                thread={thread}
                active={thread.id === activeThreadId && !proxyActive}
                onSelect={() => onSelectThread(thread.id)}
                onOpenMenu={setMenu}
              />
            ))}
            {filtered.length === 0 && query ? (
              <div className="px-3 pt-6 text-[12.5px] text-[var(--color-text-tertiary)]">
                No threads match “{query}”.
              </div>
            ) : null}
          </div>
        </div>

        <div className="no-drag px-2 py-2">
          <AccountMenu account={account} accountId={accountId} connected={connected} billing={billing}
            onSignIn={onSignIn} onLogout={onLogout} onOpenSettings={onOpenSettings} onOpenBalance={onOpenBalance} onRefreshBalance={onRefreshBalance} />
        </div>
      </aside>
      {menu ? <SidebarContextMenu
        target={menu}
        onClose={closeMenu}
        onRename={() => menu.kind === "thread" ? onRenameThread(menu.id) : onRenameFolder(menu.id)}
        onDelete={() => menu.kind === "thread" ? onDeleteThread(menu.id) : onDeleteFolder(menu.id)}
      /> : null}

      <DragOverlay>
        {draggingThread ? (
          <div className="glass-panel shadow-glass-pop rounded-[8px] px-3 py-1.5 text-[13px] text-[var(--color-text-primary)]">
            {draggingThread.title}
          </div>
        ) : null}
      </DragOverlay>
    </>
  );
}
