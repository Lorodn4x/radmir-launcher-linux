import test from "node:test";
import assert from "node:assert/strict";
import { state, applyPreparation, canPlay, mainAction, shouldAutoPrepare, runtimeLocked, defaultSettings, needsDependencyPlan } from "../frontend/js/state.js";
import { processIsRunning } from "../frontend/js/format.js";
const snapshot = (patch = {}) => ({ active: false, ready: true, environment: { launch_ready: true }, process: null, ...patch });
function reset() { Object.assign(state, { bootStatus:"ready", dependencyLoading:false, settings: { ...defaultSettings, nickname:"Test_Player", selected_server_id:"1" }, servers:[{ id:"1" }], launching:false, saving:false, error:null, updates:{status:"current", installed_build:"8.9.3"} }); applyPreparation(snapshot()); }
test("files alone cannot enable Play", () => { reset();applyPreparation(snapshot({ files_ready:true, environment:{launch_ready:false} }));assert.equal(canPlay(),false); });
test("starting or recovering session blocks a second launch and runtime switching", () => { for (const status of ["starting","running","recovering"]) { reset();applyPreparation(snapshot({ process:{status} }));assert.equal(canPlay(),false);assert.equal(runtimeLocked(),true);assert.equal(processIsRunning(state.process),true); } });
test("reopened window retains active operation and progress", () => { reset();applyPreparation(snapshot({active:true,operation_id:"op",progress:{phase:"runtime-download",percent:43}}));assert.equal(canPlay(),false);assert.equal(state.progress.percent,43);assert.equal(state.operationId,"op");assert.equal(runtimeLocked(),true); });
test("normal game exit makes Play available", () => { reset();applyPreparation(snapshot({process:{status:"exited"}}));assert.equal(canPlay(),true); });
test("known update blocks Play until user chooses to download", () => {
  reset();applyPreparation(snapshot({updates:{status:"update_available",installed_build:"8.9.3",latest_build:"8.9.4"}}));
  assert.equal(canPlay(),false);
  assert.equal(state.updates.latest_build,"8.9.4");
});
test("warm bootstrap restores build without foreground preparation", () => {
  reset();applyPreparation(snapshot({updates:{status:"checking",installed_build:"8.9.3"}}));
  assert.equal(state.build,"8.9.3");assert.equal(state.progress,null);assert.equal(state.prepareStatus,"ready");
});
test("main action exposes user initiated update and never starts automatic preparation for it", () => {
  reset();applyPreparation(snapshot({updates:{status:"update_available",installed_build:"8.9.3",latest_build:"8.9.4"}}));
  assert.deepEqual(mainAction(),{kind:"update",label:"ОБНОВИТЬ",enabled:true});assert.equal(shouldAutoPrepare(),false);
  state.settings.nickname="";state.servers=[];assert.equal(mainAction().enabled,true);
  state.process={status:"running"};assert.equal(mainAction().enabled,false);
});
test("warm current and offline installations show Play without an automatic scan", () => {
  for (const status of ["current","checking","unavailable"]) {
    reset();state.updates.status=status;assert.equal(mainAction().kind,"play");assert.equal(canPlay(),true);assert.equal(shouldAutoPrepare(),false);
  }
});
test("first install, interrupted preparation and manual repair retain an actionable button", () => {
  reset();applyPreparation(snapshot({ready:false,environment:{launch_ready:false},updates:{status:"not_installed",installed_build:null}}));
  assert.equal(mainAction().kind,"download");assert.equal(shouldAutoPrepare(),true);
  state.error={code:"cancelled"};assert.equal(mainAction().enabled,true);assert.equal(shouldAutoPrepare(),false);
  state.error=null;state.updates={status:"current",installed_build:"8.9.3"};assert.equal(mainAction().kind,"prepare");
  state.operationActive=true;assert.equal(mainAction().enabled,false);
});
test("saving settings blocks a launch until saved", () => { reset();state.saving=true;assert.equal(canPlay(),false); });
test("restarted worker retains the actionable graphics diagnostic", () => {
  reset();
  const error = {code:"missing_32bit_graphics",message:"Mesa i386 missing",log_path:"runtime.log"};
  applyPreparation(snapshot({ready:false,files_ready:true,environment:{launch_ready:false,graphics_error:error}}));
  assert.equal(state.prepareStatus,"error");assert.equal(state.error.log_path,"runtime.log");
  assert.equal(canPlay(),false);assert.equal(runtimeLocked(),false);assert.equal(needsDependencyPlan(state.error),true);
});
