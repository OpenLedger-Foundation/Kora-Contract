export interface EventSubscriptionOptions {
  intervalMs?: number;
}

export function createEventSubscription<T>(
  loader: () => Promise<T>,
  onData: (value: T) => void,
  options: EventSubscriptionOptions = {}
) {
  const intervalMs = options.intervalMs ?? 5000;
  const timer = setInterval(async () => {
    onData(await loader());
  }, intervalMs);

  return () => clearInterval(timer);
}
