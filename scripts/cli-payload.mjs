import { cp } from 'node:fs/promises';

export function copyCliPayload(source, destination) {
  return cp(source, destination, {
    recursive: true, verbatimSymlinks: true, errorOnExist: true, force: false,
  });
}
