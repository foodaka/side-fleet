// Your approved services: the ones a task's agent may pay without asking first. Kept on
// the control plane, which copies them onto each task with a budget.

import { api } from './api';

export interface ApprovedService {
	host: string;
	name?: string | null;
	/** The address its first payment went to; a payment elsewhere asks first. */
	recipient?: string | null;
}

export interface PayPolicy {
	/** `approved`: any other service asks first. `any`: any service (under the threshold). */
	mode: 'approved' | 'any';
	approved: ApprovedService[];
}

/** A service from the public MPP catalog. */
export interface CatalogService {
	id: string;
	name: string;
	host: string;
	description?: string;
	categories?: string[];
	url?: string;
}

export const getPolicy = () => api<PayPolicy>('/v1/pay-policy');
export const savePolicy = (p: PayPolicy) => api<PayPolicy>('/v1/pay-policy', { method: 'PUT', body: JSON.stringify(p) });
export const catalog = () => api<{ services: CatalogService[] }>('/v1/pay-services').then((r) => r.services);

/** Add a service to your approved ones (kept with the address you approved, if any). */
export async function allow(service: ApprovedService): Promise<PayPolicy> {
	const p = await getPolicy();
	const rest = p.approved.filter((s) => s.host.toLowerCase() !== service.host.toLowerCase());
	return savePolicy({ ...p, approved: [...rest, service] });
}
