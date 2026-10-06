import React, { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import { FundingPanel } from "../../src/renderer/src/components/FundingPanel";
import "../../src/renderer/src/index.css";

function Fixture() {
  const [account, setAccount] = useState("account-a");
  useEffect(() => {
    const switchAccount = () => setAccount("account-b");
    window.addEventListener("axiom-test-switch-account", switchAccount);
    return () => window.removeEventListener("axiom-test-switch-account", switchAccount);
  }, []);
  const address = `u1${"a".repeat(180)}`;
  return <main className="mx-auto max-w-2xl p-6">
    <button onClick={() => setAccount("account-b")}>Switch account</button>
    <FundingPanel key={account} accountId={account} connected payment={{network: "mainnet", asset: "ZEC", conversion_status: "none",
      state: "ready", valuation_enabled: true, address, payment_uri: `zcash:${address}`, monitoring_status: "ready",
      required_confirmations: "10", confirmed_zatoshis: "0", confirming_zatoshis: "0", review_required: false, deposits: []}}
      onRefreshBilling={async () => {}} />
  </main>;
}
createRoot(document.getElementById("root")!).render(<Fixture />);
