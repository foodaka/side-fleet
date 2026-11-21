// Your passkey wallet for agent budgets (Tempo testnet). The passkey is the wallet's
// root key: it stays on your device, and signing a task's budget asks for it like a
// sign-in. What is kept here (and with your account) is public: the passkey's id, its
// public key and the wallet's address. viem loads only when a wallet is used.

/** Tempo's testnet, which is all budgets pay on for now. */
const RPC = 'https://rpc.moderato.tempo.xyz';
/** pathUSD, the testnet dollar (6 decimals: base units are micros). */
export const PATH_USD = '0x20c0000000000000000000000000000000000000';

export interface PasskeyWallet {
	/** The passkey's credential id. */
	id: string;
	/** Its P-256 public key, hex. */
	publicKey: `0x${string}`;
	/** The wallet's address, derived from the public key. */
	address: `0x${string}`;
}

/** A budget's access key, as the worker offered it. */
export interface OfferedKey {
	address: string;
	chainId: number;
	token: string;
	expiresAt: string;
}

export const passkeysWork = () => typeof window !== 'undefined' && !!window.PublicKeyCredential && window.isSecureContext;

/** Make a passkey and the wallet it controls. */
export async function createWallet(label: string): Promise<PasskeyWallet> {
	const { Account, WebAuthnP256 } = await import('viem/tempo');
	const credential = await WebAuthnP256.createCredential({ label });
	const account = Account.fromWebAuthnP256({ id: credential.id, publicKey: credential.publicKey });
	return { id: credential.id, publicKey: credential.publicKey, address: account.address };
}

/**
 * Sign a task's key for its budget: it may spend up to `budgetMicros` of the key's token
 * until the key's expiry, and nothing else. Asks for the passkey. Returns the signed
 * authorization, RLP-encoded, for the worker.
 */
export async function signBudget(wallet: PasskeyWallet, key: OfferedKey, budgetMicros: number): Promise<`0x${string}`> {
	const [{ Account }, { KeyAuthorization }] = await Promise.all([import('viem/tempo'), import('ox/tempo')]);
	const account = Account.fromWebAuthnP256({ id: wallet.id, publicKey: wallet.publicKey });
	const signed = await Account.signKeyAuthorization(account, {
		chainId: BigInt(key.chainId),
		key: { accessKeyAddress: key.address as `0x${string}`, keyType: 'secp256k1' },
		expiry: Math.floor(Date.parse(key.expiresAt) / 1000),
		limits: [{ token: key.token as `0x${string}`, limit: BigInt(budgetMicros) }]
	});
	return KeyAuthorization.serialize(signed);
}

async function rpc<T>(method: string, params: unknown[]): Promise<T> {
	const r = await fetch(RPC, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ jsonrpc: '2.0', id: 1, method, params }) });
	const body = await r.json();
	if (body.error) throw new Error(body.error.message ?? `${method} failed`);
	return body.result as T;
}

/** The wallet's test dollars, in micros. */
export async function balance(address: string): Promise<number> {
	const data = '0x70a08231' + address.slice(2).toLowerCase().padStart(64, '0'); // balanceOf(address)
	const out = await rpc<string>('eth_call', [{ to: PATH_USD, data }, 'latest']);
	return Number(BigInt(out || '0x0'));
}

/** Ask the testnet faucet for test dollars. */
export async function fund(address: string): Promise<void> {
	await rpc('tempo_fundAddress', [address]);
}
