import { Fingerprint, Mail, Wallet } from "lucide-react";
import type { LoginMethod } from "../api/types";
export function GoogleIcon() {
  return <svg aria-hidden="true" viewBox="0 0 24 24" width="19" height="19"><path fill="currentColor" d="M21.8 12.2c0-.7-.1-1.5-.2-2.2H12v4.1h5.5a4.8 4.8 0 0 1-2.1 3.1v2.7h3.4c2-1.9 3-4.5 3-7.7ZM12 22c2.8 0 5.2-.9 6.9-2.5l-3.4-2.7c-.9.6-2.1 1-3.5 1-2.7 0-5-1.8-5.8-4.3H2.7v2.8A10 10 0 0 0 12 22ZM6.2 13.5a6 6 0 0 1 0-3.8V6.9H2.7a10 10 0 0 0 0 9.4l3.5-2.8ZM12 5.7c1.5 0 2.9.5 4 1.6l3-3A10 10 0 0 0 2.7 6.9l3.5 2.8C7 7.2 9.3 5.7 12 5.7Z"/></svg>;
}
export function MethodIcon({ method }: { method: LoginMethod }) {
  if (method === "google") return <GoogleIcon />;
  const Icon = method === "passkey" ? Fingerprint : method === "password" ? Mail : Wallet;
  return <Icon size={18} aria-hidden="true" />;
}
