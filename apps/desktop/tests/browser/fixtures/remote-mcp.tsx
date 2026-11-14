import { useState } from "react";
import { createRoot } from "react-dom/client";
import type { ClientSessionState, DesktopMcpRequest, DesktopMcpResponse, DesktopMcpServer } from "@axiom/axiom-acp-client";
import type { AgentSelection } from "@axiom/chat-ui/AgentModeControl";
import { ChatView } from "../../../src/renderer/src/components/ChatView";
import { McpConnectionsPanel } from "../../../src/renderer/src/components/McpConnectionsPanel";
import { securityEvidence } from "../../../../../fixtures/desktop-security.mts";
import "../../../src/renderer/src/index.css";

const remote: DesktopMcpServer = { name:"project_tracker", displayName:"Project tracker", command:"", args:[], remote:{url:"https://mcp.example.com/mcp",auth:"oauth"},enabled:true,environmentKeys:[],credentialId:"fixture-grant",status:"tested",tools:[
  {name:"mcp__project_tracker__search_issues",description:"Find issues in your projects.",schemaHash:"search"},
  {name:"mcp__project_tracker__create_issue",description:"Create an issue in a project.",schemaHash:"create"},
] };
const local: DesktopMcpServer = { name:"local_files",displayName:"Local files",command:"/usr/local/bin/files-mcp",args:[],enabled:true,environmentKeys:[],status:"tested",tools:[
  {name:"mcp__local_files__read_file",description:"Read a file on this computer.",schemaHash:"read"},
] };
let snapshot: DesktopMcpResponse = {remoteSupported:true,revision:1,servers:[remote,local],selectedTools:[]};
const calls: DesktopMcpRequest[] = [];
function Harness() {
  const [view,setView] = useState("chat");
  const [config,setConfig] = useState(snapshot);
  const [selection,setSelection] = useState<AgentSelection>({enabled:false,permission:"approve_commands",workingDirectory:null,mcpTools:[remote.tools[0]!.name]});
  const [session,setSession] = useState<ClientSessionState>({sessionId:"fixture",title:"Project review",cwd:"",settings:{model:"test-model",thinkingLevel:"medium",permissionProfile:"none"},security:{state:"verified"},securityEvidence:securityEvidence(),modes:[],currentModeId:null,configOptions:[],running:false,needsResync:false,threadRevision:1,lastTimelineSequence:2,interactions:[],timeline:[
    {id:"question",kind:"user",text:"Find open issues related to customer onboarding.",status:"completed"},
    {id:"tool",kind:"tool",text:"",tool:{callId:"search",name:remote.tools[0]!.name,title:remote.tools[0]!.name,kind:"other",status:"completed",started:true,input:{query:"customer onboarding",status:"open"},content:[{type:"text",text:"Found 3 open issues."}],locations:[],mcpSource:{server:"Project tracker",destination:"https://history.example.com/mcp"}}},
    {id:"answer",kind:"assistant",text:"I found three open issues related to customer onboarding. Two concern the welcome flow, and one concerns account setup.",status:"completed"},
  ]});
  Object.assign(window,{axiomDesktop:{agent:{desktopMcp:async(request:DesktopMcpRequest)=>{
    calls.push(structuredClone(request));
    const action=request.action;
    if(action.kind!=="list" && request.expectedRevision!==snapshot.revision) throw new Error("MCP settings changed");
    if(action.kind==="save") {
      const {env,bearerToken,...input}=action.server;
      const previous=snapshot.servers.find(s=>s.name===input.name);
      const saved:DesktopMcpServer={...input,command:input.command??"",args:input.args??[],enabled:input.enabled,environmentKeys:Object.keys(env??{}),credentialId:bearerToken?"new-token":previous?.credentialId??null,tools:[],status:"untested"};
      snapshot={...snapshot,revision:snapshot.revision+1,servers:[...snapshot.servers.filter(s=>s.name!==saved.name),saved]};
    } else if(action.kind==="test") snapshot={...snapshot,revision:snapshot.revision+1,servers:snapshot.servers.map(s=>s.name===action.name?{...s,status:"tested",tools:[{name:`mcp__${s.name}__lookup`,description:"Look up a project.",schemaHash:"fixture"}]}:s)};
    else if(action.kind==="disconnect") snapshot={...snapshot,revision:snapshot.revision+1,servers:snapshot.servers.map(s=>s.name===action.name?{...s,status:"needs_auth",credentialId:null,enabled:false,tools:[]}:s)};
    else if(action.kind==="delete") snapshot={...snapshot,revision:snapshot.revision+1,servers:snapshot.servers.filter(s=>s.name!==action.name)};
    else if(action.kind==="connect") snapshot={...snapshot,revision:snapshot.revision+1,servers:snapshot.servers.map(s=>s.name===action.name?{...s,credentialId:"fixture-oauth",status:"untested"}:s)};
    setConfig(snapshot);return structuredClone(snapshot);
  }}},__remoteMcpTest:{calls:()=>calls,snapshot:()=>snapshot,selection:()=>selection,setView,setSession,
    setTool:(changes:Record<string,unknown>)=>setSession(current=>({...current,timeline:current.timeline.map(item=>item.tool?{...item,tool:{...item.tool,...changes}}:item)}))}});
  return <div className="h-screen bg-[var(--color-bg-base)] p-4 text-[var(--color-text-primary)]">
    <main className="relative h-full overflow-hidden rounded-[22px] bg-[var(--surface-stage)] shadow-glass-card">
      {view==="settings"?<div className="mx-auto h-full max-w-[760px] overflow-y-auto p-8"><h1 className="mb-2 text-[22px] font-medium">MCP connections</h1><p className="mb-7 text-[13px] text-[var(--color-text-tertiary)]">Connect local processes and remote services to your conversations.</p><McpConnectionsPanel/></div>:<>
        <header className="absolute inset-x-0 top-0 z-10 flex h-12 items-center justify-center text-[13px] text-[var(--color-text-secondary)]">Project review</header>
        <ChatView session={session} draft="" onDraftChange={()=>{}} onSubmit={()=>{}} onStop={()=>{}} isStreaming={false}
          models={[{id:"test-model",label:"DeepSeek V4 Flash",shortLabel:"DeepSeek V4 Flash",providerId:"near",providerLabel:"NEAR",model:"test-model",supportedReasoningEfforts:[],inputPriceMicrousdPerMillionTokens:null,outputPriceMicrousdPerMillionTokens:null}]}
          selectedModelId="test-model" modelStatusLabel="" onModelChange={()=>{}} reasoningEffort="medium" onReasoningEffortChange={()=>{}} onVerifySecurity={()=>{}} securityScope="fixture" webEnabled={false} onWebToggle={()=>{}}
          agentControl={{scope:"fixture",selection,mcpServers:config.servers,onApply:async next=>setSelection(next),onChooseDirectory:async()=>null}} />
      </>}
    </main>
  </div>;
}
createRoot(document.getElementById("root")!).render(<Harness/>);
