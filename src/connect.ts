// The connect-and-recover flow shared by terminals and SFTP.
import * as api from "./api";
import type { CommandError, Host } from "./api";
import { confirmAction, promptCredential } from "./dialogs";
import { displayName, plain } from "./format";

interface Credential {
  secret: string;
  /** Save to the keychain once the server accepts it. */
  remember: boolean;
}

/**
 * Calls `attempt` until it succeeds, asking the user to resolve host-key and
 * credential failures in between. Resolves to null when the user gives up or
 * the failure can't be recovered; `report` receives the reason.
 */
export async function connectWithRecovery<T>(
  host: Host,
  attempt: (secret: string | undefined) => Promise<T>,
  report: (message: string) => void,
): Promise<T | null> {
  let credential: Credential | undefined;
  for (;;) {
    try {
      const result = await attempt(credential?.secret);
      // Saved only after the server accepted it, so a typo is never stored. If
      // saving fails, the next connection simply asks again.
      if (credential?.remember) api.setHostSecret(host.id, credential.secret).catch(() => {});
      return result;
    } catch (error) {
      const failure = api.asCommandError(error);
      let retry: { credential?: Credential } | null;
      try {
        retry = await recover(host, failure, credential);
      } catch (recoveryError) {
        report(api.asCommandError(recoveryError).message);
        return null;
      }
      if (retry === null) {
        report(failure.message);
        return null;
      }
      credential = retry.credential;
    }
  }
}

/** Resolves to the next attempt's credential, or null to give up. */
async function recover(
  host: Host,
  failure: CommandError,
  credential: Credential | undefined,
): Promise<{ credential?: Credential } | null> {
  switch (failure.kind) {
    case "unknownHostKey": {
      const key = failure.hostKey!;
      const trusted = await confirmAction({
        title: "Unknown host key",
        message: `This is the first connection to ${plain(`${key.host}:${key.port}`)}. Only continue if this fingerprint matches the server's host key.`,
        detail: `${key.algorithm}\n${key.fingerprint}`,
        confirmLabel: "Trust and connect",
      });
      if (!trusted) return null;
      await api.trustHostKey(key);
      return { credential };
    }
    case "passwordRequired":
    case "passphraseRequired":
    case "authFailed": {
      if (failure.kind === "authFailed" && host.auth.type !== "password") return null;
      const passphrase = failure.kind === "passphraseRequired";
      const keyPath = host.auth.type === "key" ? host.auth.path : "the key";
      const entered = await promptCredential({
        title: passphrase ? "Key passphrase" : "Password",
        message:
          failure.kind === "authFailed"
            ? `Authentication to ${plain(displayName(host))} failed. Try another password.`
            : passphrase
              ? `Enter the passphrase for ${plain(keyPath)}.`
              : `Enter the password for ${plain(`${host.username}@${host.hostname}`)}.`,
        confirmLabel: "Connect",
      });
      return entered === null ? null : { credential: entered };
    }
    default:
      return null;
  }
}
