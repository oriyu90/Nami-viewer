/**
 * Entry point for `node --test tests/frontend/`.
 *
 * Node 24 resolves a directory argument through the module loader, so this
 * file simply imports the individual suites. They can still be run directly
 * with `node --test tests/frontend/*.test.mjs`.
 */

import './core.test.mjs';
import './i18n.test.mjs';
