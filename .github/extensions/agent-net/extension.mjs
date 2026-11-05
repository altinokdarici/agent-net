import { joinSession } from "@github/copilot-sdk/extension";

const { start } = await import(new URL("../../../src/extension.ts", import.meta.url).href);
await start(joinSession);
