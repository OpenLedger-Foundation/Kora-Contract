export interface IndexedEvent<T = Record<string, unknown>> {
  cursor: string;
  payload: T;
}

export function createQueryLayer<T>(events: IndexedEvent<T>[]) {
  return {
    all: () => events,
    first: () => events[0] ?? null,
  };
}
