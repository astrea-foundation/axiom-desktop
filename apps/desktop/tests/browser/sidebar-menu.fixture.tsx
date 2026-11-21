import React, { useState } from "react";
import { createRoot } from "react-dom/client";
import { SidebarContextMenu, type SidebarMenuTarget } from "../../src/renderer/src/components/SidebarContextMenu";
import "../../src/renderer/src/index.css";

/** A thread list and a transcript that scroll independently, as in the app. */
function Fixture() {
  const [menu, setMenu] = useState<SidebarMenuTarget | null>(null);
  const [action, setAction] = useState("");
  return <main className="flex h-[300px] gap-4 p-4">
    <div data-thread-list className="h-full w-60 overflow-y-auto"><div style={{ height: 900 }}>
      <button type="button" aria-label="Actions for Test thread" aria-haspopup="menu" aria-expanded={!!menu} onClick={(event) => {
        const trigger = event.currentTarget;
        const { right, bottom } = trigger.getBoundingClientRect();
        setMenu(menu ? null : { kind: "thread", id: "thread", trigger, x: right, y: bottom, fromMenuButton: true });
      }}>⋮</button>
    </div></div>
    <div data-transcript className="h-full flex-1 overflow-y-auto"><div style={{ height: 4000 }}>Reply text</div></div>
    <output>{action}</output>
    {menu ? <SidebarContextMenu target={menu} onClose={() => setMenu(null)} onRename={() => setAction("rename")} onDelete={() => setAction("delete")} /> : null}
  </main>;
}
createRoot(document.getElementById("root")!).render(<Fixture />);
