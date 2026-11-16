import { AxiomStateStore } from "../src/state.js";

// Every shared state view built during these tests is compared with a complete
// copy of the store and frozen, so a missed change mark or a consumer mutation
// fails the test that caused it.
AxiomStateStore.verifyViews = true;
