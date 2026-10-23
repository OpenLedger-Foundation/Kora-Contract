import { DependencyList, useCallback, useEffect, useRef, useState } from 'react';
import { KoraUserError, mapSdkError } from '../sdk/errors';

export type ResourceState<T> =
  | { status: 'loading' }
  | { status: 'ready'; data: T }
  | { status: 'error'; error: KoraUserError };

/**
 * Loads async data with explicit loading / ready / error states. Results from
 * a superseded load (deps changed, component unmounted) are ignored.
 */
export function useAsyncResource<T>(
  load: () => Promise<T>,
  deps: DependencyList,
  initial?: T,
): { state: ResourceState<T>; reload: () => void } {
  const [state, setState] = useState<ResourceState<T>>(
    initial !== undefined ? { status: 'ready', data: initial } : { status: 'loading' },
  );
  const generation = useRef(0);
  const skipFirst = useRef(initial !== undefined);

  const run = useCallback(() => {
    const current = ++generation.current;
    setState({ status: 'loading' });
    load().then(
      (data) => {
        if (generation.current === current) setState({ status: 'ready', data });
      },
      (error: unknown) => {
        if (generation.current === current) setState({ status: 'error', error: mapSdkError(error) });
      },
    );
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, deps);

  useEffect(() => {
    if (skipFirst.current) {
      skipFirst.current = false;
    } else {
      run();
    }
    return () => {
      generation.current += 1;
    };
  }, [run]);

  return { state, reload: run };
}
