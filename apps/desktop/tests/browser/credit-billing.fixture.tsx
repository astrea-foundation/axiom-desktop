import React, { useState } from "react";
import { createRoot } from "react-dom/client";
import { CreditBillingPanel } from "../../src/renderer/src/components/CreditBillingPanel";
import "../../src/renderer/src/index.css";
const updated = () => {};
const refresh = async () => {};
function Fixture() {
  const [account, setAccount] = useState("account-a");
  return <main className="mx-auto max-w-3xl p-5"><button onClick={() => setAccount("account-b")}>Switch account</button>
    <CreditBillingPanel key={account} accountId={account} connected onSummary={updated} onRefreshBilling={refresh} cryptoContent={<p>Choose a cryptocurrency</p>} />
  </main>;
}
createRoot(document.getElementById("root")!).render(<Fixture />);
