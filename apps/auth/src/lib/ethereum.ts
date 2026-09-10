interface EthereumProvider {
  request(args: { method: string; params?: unknown[] }): Promise<unknown>;
}

declare global {
  interface Window {
    ethereum?: EthereumProvider;
  }
}

export interface WalletConnection {
  provider: EthereumProvider;
  address: string;
  chainId: number;
}

export async function connectEthereumWallet(): Promise<WalletConnection> {
  const provider = window.ethereum;
  if (!provider) throw new Error("No compatible Ethereum wallet was found in this browser.");

  const accounts = await provider.request({ method: "eth_requestAccounts" });
  if (!Array.isArray(accounts) || typeof accounts[0] !== "string") {
    throw new Error("The wallet did not return an account.");
  }
  const address = normalizeEthereumAddress(accounts[0]);
  const rawChain = await provider.request({ method: "eth_chainId" });
  if (typeof rawChain !== "string" || !/^0x[0-9a-f]+$/i.test(rawChain)) {
    throw new Error("The wallet returned an invalid chain identifier.");
  }
  const chainId = Number.parseInt(rawChain, 16);
  if (!Number.isSafeInteger(chainId) || chainId <= 0) throw new Error("Unsupported Ethereum chain.");
  return { provider, address, chainId };
}

export async function signSiweMessage(connection: WalletConnection, message: string): Promise<string> {
  if (message.length < 20 || message.length > 8_192) throw new Error("The sign-in message is invalid.");
  const signature = await connection.provider.request({
    method: "personal_sign",
    params: [message, connection.address],
  });
  if (typeof signature !== "string" || !/^0x[0-9a-f]{130}$/i.test(signature)) {
    throw new Error("The wallet returned an invalid signature.");
  }
  return signature;
}

export function normalizeEthereumAddress(value: string): string {
  if (!/^0x[0-9a-f]{40}$/i.test(value)) throw new Error("The wallet returned an invalid address.");
  return value.toLowerCase();
}
