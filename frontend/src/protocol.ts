export type StatusKind = "available" | "out-of-stock" | "not-yet-on-sale" | "sold-out" | "unknown";
export interface Status {
  kind: StatusKind;
  label: string;
}
export interface EventView {
  itemId: string;
  name: string;
  city: string;
  venue: string;
  sale: Status;
  sellStart: string | null;
  notice: string | null;
  sessions: {
    id: string;
    name: string;
    tiers: { name: string; price: string; status: Status }[];
    error: string | null;
    notice: string | null;
  }[];
  report: string;
}
export interface QueryError {
  code: string;
  message: string;
}
export class EventError extends Error {
  constructor(
    public readonly code: string,
    message: string,
  ) {
    super(message);
  }
}
