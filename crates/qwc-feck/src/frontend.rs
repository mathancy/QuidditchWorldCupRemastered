//! The front end around the VM (`docs/FECK_VM.md`, "Front end"): layers built from the scripts' `_LayerData`, the
//! DataStore, event dispatch, per-frame actions, and the natives scripts call (`docs/feck_natives.md`). Natives
//! not ported yet are recorded in [`FrontEnd::missing`] and return 0.

use std::collections::{BTreeMap, HashMap};

use qwc_formats::flow::name_hash;
use qwc_formats::vso::Vso;

use crate::keys::Keys;
use crate::layer::{self, event, flag, Action, ActionKind, Layer, Object};
use crate::layout::{layout, Layout, Metrics, Params};
use crate::memcard::{self, MemCard};
use crate::memory::Memory;
use crate::puppet::{self, Puppet, SetLoader};
use crate::spline::{self, Spline};
use crate::text::{Strings, Value, Values};
use crate::vm::{Error, Host, Result, Vm};
use qwc_anim::control::CrtRand;

/// One DataStore entry (`0x4d6e80`): `count` elements of `stride` bytes at `addr` in VM memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    pub key: u32,
    pub kind: u32,
    pub count: u16,
    pub stride: u16,
    pub addr: u32,
}

/// Element sizes of the DataStore types (`0x5ef9f4`): U8, U16, U32, U64, S8, S16, S32, S64, FP32; type 9 (a user
/// type) gives its own stride.
pub const TYPE_SIZE: [u16; 9] = [1, 2, 4, 8, 1, 2, 4, 8, 4];

/// What the scripts asked the flow to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    /// `General_RequestExit(obj, n, finish)`: take exit `n` of the current node. `layer` is the calling object's
    /// layer; `finish` lets the screen finish its transitions without its scripts (`screen +0x64`).
    Exit { layer: usize, exit: u32, finish: bool },
    /// `General_RequestExitToNode(obj, hash, finish)`: the exit leading to that node or group.
    ExitToNode { layer: usize, hash: u32, finish: bool },
    /// `General_RequestJumpToNode(obj, hash, finish)`: straight to that node.
    JumpToNode { layer: usize, hash: u32, finish: bool },
    /// The screen's pre-transition is done (`0x4d32a0`).
    PreTransitionDone { layer: usize },
    /// The screen may leave (`0x4d32b0`).
    ReadyToExit { layer: usize },
}

/// What the natives posted to the engine's event routers (`EventSystem\Router.cpp`; a router calls every listener
/// registered on it). Named after the native that posts it; what the listeners do is not traced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineEvent {
    /// `General_FrontendEntry(on)` (`0x4d3da0`, router `0x1043bc4`). Entering also posts `FrontendScene(true)`,
    /// `ScreenFade(false)` and the six hoops' placements (router `0x1043ab8`, the hoop table of `docs/GAMEPLAY.md`,
    /// kept as [`FrontEnd::hoops`]); leaving posts `FrontendScene(false)` and a reset (router `0x104397c`).
    FrontendEntry(bool),
    /// Router `0x1043a00`: the front end's 3D scene on or off (also `General_FrontendCutSceneEntry(x)`, which
    /// posts `x == 0`, `0x4d3d70`).
    FrontendScene(bool),
    /// `General_LoadScreen(0)` (`0x4d4150`, router `0x1043964`); other arguments post nothing.
    LoadScreen,
    /// `General_SetPaused` (`0x4d41d0`, router `0x1043bbc`).
    Paused(bool),
    /// `General_TimeoutEnable(x)` (`0x4d3d50`, router `0x1043bc0`).
    TimeoutEnable(u32),
    /// `General_ChangeLanguage(hash)` (`0x4d3b30`, router `0x1043934`): the string manager switches language.
    Language(u32),
    /// `General_AudioVolumeEvent(channel, value)` (`0x4d3370`, router `0x1043938`).
    AudioVolume {
        channel: u8,
        value: u32,
    },
    /// `General_PlayResultsSpeech(a, b, c)` (`0x4d4360`, router `0x1043928`).
    ResultsSpeech([u32; 3]),
    /// `General_PlayMovie(name, flag)` (`0x4d3fd0`): the original plays it to the end inside the native.
    PlayMovie {
        name: String,
        flag: u32,
    },
    /// `General_RefreshWavyFlags` (`0x4d4140` → `0x49b6d0`): the stadium's flags are loaded again, their textures
    /// chosen by the DataStore as it is then (`qwc_render::flags`).
    RefreshWavyFlags,
    /// `General_SetAsianInput` / `General_SetNonAsianInput` (`0x4d3930`, `0x4d39b0`): the control icons' folder
    /// (resource group `0xb2` is reloaded).
    ControlIcons(String),
    /// `General_ScreenFade(on)` (`0x4d4180`, router `0x1043bd0`): the record `{on, seconds, 0}`, 2 seconds for
    /// `false` and none for `true`.
    ScreenFade(bool),
    /// `General_PlayFrontendSpeech(a, b, c)` (`0x4d42f0`, router `0x1043930`).
    FrontendSpeech([u32; 3]),
    /// `General_PlayCutSceneSpeech(a, b)` (`0x4d43a0`, router `0x1043924`).
    CutSceneSpeech([u32; 2]),
    /// `General_PlayCutScene(arg)` (`0x4d4270`): unless the cutscene manager (`0x1004970`) is busy (`0x488590`), the
    /// `FEIntro` cutscene is queued with `arg` (`0x488720`), its state set to 5 and its flags to `0xc` (bit 28 from
    /// the byte `0xf917df`). It plays on while the scripts go on; its end (`POST_PROCESS_TYPE eFE_CUTSCENE_FINISH`)
    /// reaches the layers as general event 2 (see [`FrontEnd::general`]).
    PlayCutScene(u32),
    /// `General_PlayChallengeSpeech(a, b)` (`0x4d4330`, router `0x104392c`).
    ChallengeSpeech([u32; 2]),
    /// `General_EnableWideScreen(on)` (`0x4d4440`): the display settings (`0x1016e44`, the flag at `+8`) go to
    /// router `0x1043998`.
    WideScreen(bool),
    /// `General_InitMatch` (`0x4d3cb0`, router `0x1043b4c`): whether DataStore `0x1e394bdc` (the first word) is not
    /// negative. A negative one also runs `0x450600` first, as `DeInitMatch` does.
    InitMatch(bool),
    /// `General_DeInitMatch` (`0x4d3d30`: `0x450600`, then router `0x1043b48`).
    DeInitMatch,
    /// `General_LoadSet(id)` / `General_UnloadSet(id)` (`0x4d3780`, `0x4d3750`): a load set from the table
    /// `General_LoadSetIDFromCRC` indexes, handed to the resource managers `0xfcaf40` and `0x60c560`.
    LoadSet(u32),
    UnloadSet(u32),
    /// `General_GameInitGameAudio` (`0x4d5150`: the audio manager `0xf9986c`'s `+8`).
    InitGameAudio,
    /// `General_ChangeDisplay(mode)` (`0x4d43d0`: `0x511150`, then `0x50a850` with the display settings
    /// `0x60beb8`): the Video options' choice, an index of [`FrontEnd::display_modes`] (`General_VerifyDisplay`'s).
    ChangeDisplay(u32),
}

/// The three front-end lights (`0xfa31ec`-`0xfa3258`): `General_Light_*`. Directions are normalised (a zero
/// vector stays zero).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Lights {
    pub ambient: [f32; 3],
    pub diffuse: [[f32; 3]; 3],
    pub direction: [[f32; 3]; 3],
}

/// Quidditch cards per row of the card bits (`0x668c18`): 104 in all.
pub const CARDS_PER_ROW: [u32; 15] = [10, 10, 10, 10, 4, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6];
/// The DataStore keys of the card bits, the card total and the unlock bits.
pub const KEY_CARD_BITS: u32 = 0xacfe6233;
pub const KEY_CARD_TOTAL: u32 = 0xa20daf5d;
pub const KEY_UNLOCK_BITS: u32 = 0x1527016b;

/// The hashes of the exe's native table (`0x5f1a20`, 211 entries; `docs/feck_natives.md`): the natives the
/// original's lookup (`0x4df320`) can find. An import that is none of these and no export of its own script
/// does not resolve.
const EXE_NATIVES: [u32; 211] = [
    0xbecb753e, 0xc9cc45a8, 0x50c51412, 0x2e7468af, 0xa37c655a, 0x61b0b5f6, 0x16b78560, 0x8fbed4da, 0xf10fa867,
    0x7c07a592, 0xcf38a15d, 0xba6d975f, 0x2e30e25a, 0x93a44fd7, 0x667f4d0c, 0x9c6013a7, 0xa9f7790f, 0x5e1e9c20,
    0x33c378cb, 0x43a98c44, 0xdaa0ddfe, 0xe14fef3f, 0x9648dfa9, 0x0f418e13, 0xd930679e, 0x72de57e2, 0x05d96774,
    0x9cd036ce, 0xaa7e3a40, 0xbdcf0a75, 0xb4a05db2, 0xed176a6a, 0x323fe419, 0x1a3c2a7e, 0xb15302af, 0xf902f503,
    0x1cbdb173, 0xd720d677, 0x43321273, 0xc3afa535, 0xb61fe3d0, 0x4eb7c849, 0xfb26210d, 0xcdf80287, 0x1a63b020,
    0x2d0a6a59, 0x28f77e9b, 0x77e38add, 0x70d47c4c, 0xa03f9fac, 0xff5bcc7e, 0xaa490766, 0x857e56e5, 0x5757d397,
    0x876fbb30, 0xf3d1777c, 0xccba2f15, 0xfc49fc3c, 0xf748fe80, 0x0a8fc286, 0x7e310eca, 0x29188c42, 0x8194e813,
    0x8cb89636, 0xe0efa400, 0xe6d71212, 0xc07a5a79, 0x366c956e, 0x389d2bf4, 0x99f2bde5, 0x2b6ffe5d, 0x67469364,
    0x3e951f8f, 0x93d40b6d, 0x74392346, 0x824d70cc, 0x8e1315ae, 0x942091ae, 0xd2080811, 0x7867eb2f, 0x11e4f9a4,
    0xa9f6da46, 0x706c6fe3, 0x6e43d8aa, 0x41e8352d, 0x41b440dc, 0xe1570dbe, 0x579695a0, 0xbc65347c, 0x95f58b60,
    0x535fba03, 0x66041c98, 0xc3af2ce6, 0x57d2c6b1, 0x6289602a, 0x2743f2a3, 0x142d3999, 0x0095ba22, 0xcb18fa8e,
    0xfe435c15, 0x56cf1da4, 0xcf95863c, 0xface20a7, 0xb4d24a7e, 0x0a028ed0, 0x0b930a71, 0xbf82fa90, 0x598aed38,
    0x51f0bb17, 0x4911dfda, 0xa79a636e, 0xd7dbf55d, 0x6a6db2e7, 0xf827c9b9, 0x849d698f, 0x2b898784, 0x7bda7676,
    0xb1611c6e, 0x731ebbf4, 0x5dca416a, 0xfd2fdd87, 0x0149af81, 0x32aee7af, 0x45a9d739, 0xdca08683, 0x20103bdb,
    0x57170b4d, 0xce1e5af7, 0x335fdd98, 0x4db054d8, 0x3ab7644e, 0xa3be35f4, 0x051bb0c5, 0x721c8053, 0xeb15d1e9,
    0x39822a68, 0x87f0b077, 0x4b406aec, 0x8ea35429, 0x9b6f6c47, 0xf85d6976, 0x3e9ef564, 0x97c60766, 0xfd6247ee,
    0xe341aa17, 0xebd71539, 0xe98a7b24, 0xc9f19505, 0xdf88f8fd, 0x3a706dbd, 0xad3fab1c, 0x36c741a6, 0xc91a7efb,
    0xa6bd0e03, 0x5f861a86, 0x953b6f60, 0x34661ee7, 0x3b151b72, 0x0462d91d, 0xa6049946, 0x055a6b16, 0xde784913,
    0xbccc1bc3, 0x518ac7f3, 0x0c3600c2, 0xd73ce7e4, 0xa72e99b7, 0x029f2403, 0x1fc68258, 0x5e575c3d, 0x803d82cc,
    0x764cf55a, 0x72ed43a3, 0x7f79b9b4, 0xc636af30, 0xbad95323, 0x64f9ce90, 0x1a3d0b3b, 0x82710297, 0xe5a136d8,
    0xd8e645b9, 0xa478a025, 0x0b8ea0ae, 0x6d16d7fd, 0xe6ba8c62, 0x274f0507, 0x07b16242, 0xd95d0834, 0xd23283b7,
    0x40bc3633, 0x3402fa7f, 0x86ceaa38, 0x94306a7f, 0xc32d122f, 0x37b03cb4, 0x27aac27c, 0x1c9246b6, 0x9532a309,
    0xf13848e9, 0x9950df82, 0x341fa002, 0x05b032f5, 0xaa975b74, 0x111a8519, 0xbcaf61fd, 0xfd54faca, 0x674be9d6,
    0xb3659513, 0x6a7e8860, 0xb1a9a1dd, 0x6462fa0b,
];

/// The natives this host implements (by name); everything else is counted in `missing`.
const NATIVES: &[&str] = &[
    "LayerObj_SetX",
    "LayerObj_SetY",
    "LayerObj_SetZ",
    "LayerObj_SetW",
    "LayerObj_SetH",
    "LayerObj_GetX",
    "LayerObj_GetY",
    "LayerObj_GetZ",
    "LayerObj_GetW",
    "LayerObj_GetH",
    "LayerObj_GetFlags",
    "LayerObj_SetFlag",
    "LayerObj_ResetFlag",
    "LayerObj_GetObjectID",
    "LayerObj_SetPosition",
    "LayerObj_SetRGBA",
    "LayerObj_SetR",
    "LayerObj_SetG",
    "LayerObj_SetB",
    "LayerObj_SetA",
    "LayerObj_GetRotX",
    "LayerObj_GetRotY",
    "LayerObj_GetRotZ",
    "LayerObj_SetRot",
    "LayerObj_SetRotX",
    "LayerObj_SetRotY",
    "LayerObj_SetRotZ",
    "LayerObj_GetObject",
    "LayerObj_GetChildObject",
    "LayerObj_GetSiblingObject",
    "LayerObj_GetParentObject",
    "LayerObj_GetChildCount",
    "LayerObj_GetUserParam",
    "LayerObj_KillAction",
    "LayerObj_AddAction_Alpha",
    "LayerObj_AddAction_Colour",
    "LayerObj_AddAction_Position",
    "LayerObj_AddAction_Timer",
    "LayerObj_AddAction_Size",
    "LayerObj_Sprite_AddAction_Animate",
    "LayerObj_GetCalculatedExtents",
    "LayerObj_ActivateWipe",
    "LayerObj_AddWipeWatcher",
    "LayerObj_DeleteWipeWatcher",
    "GBALink_SendQCardPacket",
    "General_IsPlayersControllerPresent",
    "DataStore_CreateEntry",
    "DataStore_GetU8",
    "DataStore_GetU16",
    "DataStore_GetU32",
    "DataStore_GetS8",
    "DataStore_GetS16",
    "DataStore_GetS32",
    "DataStore_GetFP32",
    "DataStore_GetUserType",
    "DataStore_SetU8",
    "DataStore_SetU16",
    "DataStore_SetU32",
    "DataStore_SetS8",
    "DataStore_SetS16",
    "DataStore_SetS32",
    "DataStore_SetFP32",
    "DataStore_SetUserType",
    "FECK_CalcCRC",
    "FECK_CalcCRCFromCHAR",
    "FECK_CheckCRC",
    "FECK_Printf",
    "General_FindObjectInLayer",
    "General_FindLayer",
    "General_GetPlatformType",
    "General_LoadSetIDFromCRC",
    "General_RequestExit",
    "General_RequestExitToNode",
    "General_RequestJumpToNode",
    "General_PreTransitionDone",
    "General_ReadyToExit",
    "General_SendAudioEvent",
    "LayerObj_Text_GetStringID",
    "LayerObj_Text_SetStringID",
    "LayerObj_Text_SetStringIDWithIndex",
    "LayerObj_Text_ForceReformatting",
    "LayerObj_Text_SetEffectFlag",
    "LayerObj_Text_ResetEffectFlag",
    "LayerObj_Menu_GetCurrentSelection",
    "LayerObj_Menu_GetCurrentSelectionIndex",
    "LayerObj_Menu_SetStartSelectionIndex",
    "LayerObj_Menu_SetCurrentSelection",
    "LayerObj_Menu_SetCurrentSelectionIndex",
    "LayerObj_Slider_SetValue",
    "LayerObj_SetInputBlock",
    "LayerObj_Sprite_SetResourceID",
    "LayerObj_AnimatedSprite_ResetAnimation",
    "General_IsPaused",
    "General_ExecuteCLICommand",
    "General_EnableStadiumRender",
    "General_FrontendEntry",
    "General_FrontendCutSceneEntry",
    "General_LoadScreen",
    "General_SetPaused",
    "General_TimeoutEnable",
    "General_ChangeLanguage",
    "General_AudioVolumeEvent",
    "General_PlayResultsSpeech",
    "General_PlayMovie",
    "General_RefreshWavyFlags",
    "General_SetAsianInput",
    "General_SetNonAsianInput",
    "General_Light_SetAmbientColour",
    "General_Light_SetDiffuseColour_0",
    "General_Light_SetDiffuseColour_1",
    "General_Light_SetDiffuseColour_2",
    "General_Light_SetDirection_0",
    "General_Light_SetDirection_1",
    "General_Light_SetDirection_2",
    "General_EnableForceFeedback",
    "General_BlockInput",
    "General_TotalQuidditchCards",
    "General_SetQuidditchCardTexture",
    "General_SetLoadingController",
    "General_VerifyDisplay",
    "General_NoAxesPlayerControls",
    "General_SetDepthOfField",
    "General_SetScreenOffsets",
    "GBALink_Reset",
    "rand",
    "srand",
    "General_GetControlKey",
    "General_SetControlKey",
    "General_GetRawKey",
    "General_IsKeyFree",
    "General_IsKeyTypeCompatible",
    "General_SaveKeyBindings",
    "General_LoadKeyBindings",
    "Memcard_EnableMemoryCards",
    "Memcard_RegisterMessageCB",
    "Memcard_RegisterStateCB",
    "Memcard_Reply",
    "Memcard_ChangeState",
    "Memcard_ActivateSlot",
    "Memcard_PrepareSlot",
    "Memcard_DeleteSlot",
    "Memcard_SetCheckingMessageDisplay",
    "Memcard_CheckCard",
    "Memcard_SetSaveGameName",
    "GBALink_IsConnected",
    "GBALink_SniffQCardCancelPacket",
    "GBALink_ReceiveQCardPacket",
    "LayerObj_Sprite_GetResourceID",
    "LayerObj_AnimatedSprite_GotoLabel",
    "LayerObj_AnimatedSprite_SetFrame",
    "LayerObj_AnimatedSprite_SetResourceID",
    "LayerObj_AnimatedSprite_GetResourceID",
    "LayerObj_Model_SetResource",
    "LayerObj_Model_SetRotationMode",
    "LayerObj_QuidditchCard_SetRotationMode",
    "LayerObj_Camera_SetInterest",
    "LayerObj_Camera_SetFocusInterest",
    "LayerObj_Particle_SetDesc",
    "LayerObj_Menu_ForceChildUpdate",
    "FECK_CalcCRCFromWCHAR",
    "General_GetBuildType",
    "General_GetNumPadsConnected",
    "General_Light_GetAmbientColour",
    "General_Light_GetDirection_0",
    "General_Light_GetDirection_1",
    "General_Light_GetDirection_2",
    "General_Light_GetDiffuseColour_0",
    "General_Light_GetDiffuseColour_1",
    "General_Light_GetDiffuseColour_2",
    "General_LoadSet",
    "General_UnloadSet",
    "General_PuppetHack",
    "General_InitMatch",
    "General_DeInitMatch",
    "General_SetExitRequestFlag",
    "General_ScreenFade",
    "General_Mudlark",
    "General_PlayFrontendSpeech",
    "General_PlayCutSceneSpeech",
    "General_PlayCutScene",
    "General_PlayChallengeSpeech",
    "General_ChangeDisplay",
    "General_GetCurrentDisplaymode",
    "General_EnableWideScreen",
    "General_MoveScreenOffset",
    "General_MoveScreenOffsetUp",
    "General_MoveScreenOffsetDown",
    "General_MoveScreenOffsetLeft",
    "General_MoveScreenOffsetRight",
    "General_ToggleInputOnOff",
    "General_SetQuidditchCard",
    "General_UseRenderToTexture",
    "General_GameInitGameAudio",
    "GBALink_GetTimeSinceLastKeepAlive",
    "LayerObj_Model_AddAction_Spline",
    "LayerObj_QuidditchCard_AddAction_Spline",
    "LayerObj_Camera_AddAction_Spline",
    "LayerObj_Sprite_AddAction_Spline",
    "LayerObj_Puppet_AddAction_Spline",
    "LayerObj_Puppet_AddAction_RelativeSpline",
    "LayerObj_Puppet_PlayAnim",
    "LayerObj_Puppet_PlayAnimCB",
    "LayerObj_Puppet_PlayAnimFlags",
    "LayerObj_Puppet_PlayAnimCBFlags",
    "LayerObj_Puppet_PlayAnimCBID",
    "LayerObj_Puppet_PlayAnimOverlay",
    "LayerObj_Puppet_PlayAnimOverlayCB",
    "LayerObj_Puppet_FadeDownSecondaryAnims",
    "LayerObj_Puppet_FadeUpSecondaryAnims",
    "LayerObj_Puppet_LookAtPosition",
];

/// Not the original's: a sprite the remaster adds to a layer ([`FrontEnd::additions`], from a layout file's
/// `adds`). It becomes a real object of the layer, so the walk draws it, the [`crate::draw::Adjust`]s of its id
/// move, scale, pin and hide it, and the canvas picks it; the scripts never look for its id, so it is still.
#[derive(Clone, Debug, PartialEq)]
pub struct SpriteAdd {
    /// Its object id (the hash of its name): the key its changes go by.
    pub id: u32,
    /// The minted resource of its picture ([`crate::draw::IMAGE_SCRIPT`]; the host's assets map it to a file).
    pub image: u32,
    /// Where it stands, as a script would place it (from its parent).
    pub pos: [f32; 2],
    /// Drawn this size; `[0, 0]` takes the image's own.
    pub size: [u16; 2],
    /// The id of the layer object it hangs under (moved and coloured with it); 0 for a layer root.
    pub parent: u32,
    /// The overlay's material layer it is drawn in.
    pub order: u32,
    /// The id of an animated sprite of the layer whose animation it copies (frame program, frame count, material,
    /// scales and origin), run on its own from the start; 0 for a still sprite.
    pub animate_like: u32,
}

pub struct FrontEnd {
    pub objects: Vec<Object>,
    pub layers: Vec<Layer>,
    pub datastore: Vec<Entry>,
    /// The load-set table (`0x68d530`, 371 slots of name hashes, slot 0 = none).
    pub load_sets: Vec<u32>,
    /// `FECK_Printf` output.
    pub log: Vec<String>,
    pub audio_events: Vec<u32>,
    pub requests: Vec<Request>,
    /// Console commands the scripts ran (`General_ExecuteCLICommand`).
    pub commands: Vec<String>,
    /// Per layer: input blocked (`layer +0x8c`, `LayerObj_SetInputBlock`).
    pub input_blocked: Vec<bool>,
    /// `General_IsPaused` (`[0x105ff38]`).
    pub paused: bool,
    /// `General_EnableStadiumRender` (the render flags it sets, `0x4d4470`: `[0xfd2f28]` and the stadium's).
    pub stadium_render: bool,
    /// `General_SetQuidditchCardTexture(folder, name)` (`0x4d5050`): the texture the material
    /// `FE_TeamTent_VCPC.card_window` is drawn with, as the folder the load looks in and the texture's name; `None`
    /// (an empty name) is the engine's default.
    pub card_texture: Option<(String, String)>,
    /// The front end's 3D scene, as the last [`EngineEvent::FrontendScene`] left it.
    pub scene: bool,
    /// The six hoops are placed: `General_FrontendEntry(on)` posts them (`0x4d3da0`, the match's table at the goal
    /// height, `qwc_sim::shooting::hoops`) and the hoop renderer keeps them; nothing in the front end takes them away.
    pub hoops: bool,
    /// Posted to the engine, in order ([`EngineEvent`]).
    pub engine: Vec<EngineEvent>,
    pub lights: Lights,
    /// `General_EnableForceFeedback` (`0x61f76c`, `0x61f76d`).
    pub force_feedback: bool,
    /// The scripts' generator (`rand`/`srand`, `[0x5f1984]`, starting at 1).
    pub rand_seed: u32,
    /// `General_ChangeLanguage`'s current language (0 until one is chosen).
    pub language: u32,
    /// The display modes `General_VerifyDisplay` checks: (width, height, bits per pixel). The host fills it.
    pub display_modes: Vec<(u32, u32, u32)>,
    /// `General_GetCurrentDisplaymode` (`[0xf91514]`). **[I]** the host sets it.
    pub display_mode: u32,
    /// `General_EnableWideScreen` (`0x1016e4c`).
    pub widescreen: bool,
    /// `General_PuppetHack` (`[0xf9173c]`).
    pub puppet_hack: u32,
    /// `General_SetExitRequestFlag` (`[0x60bed8]`, never cleared by the natives).
    pub exit_request: bool,
    /// `General_ToggleInputOnOff` (`[0x5f1980]`, 1 at start).
    pub input_toggle: u8,
    /// `General_UseRenderToTexture` (`[0x5d01a5]`, 1 at start).
    pub render_to_texture: bool,
    /// The PC key bindings (`qwc_feck::keys`).
    pub keys: Keys,
    /// The pads and the two front-end controllers (`qwc_feck::pad`); the host fills `pads.words`.
    pub pads: crate::pad::Pads,
    /// The memory-card manager (`qwc_feck::memcard`); its `io` is the save file.
    pub memcard: MemCard,
    /// The particle pool (`qwc_feck::particles`).
    pub particles: crate::particles::Particles,
    /// Puppets by object (`qwc_feck::puppet`), made as their layers build when `anim_sets` can load their clips.
    pub puppets: BTreeMap<usize, Puppet>,
    pub anim_sets: Option<SetLoader>,
    /// Clip names puppets were asked to play that their sets do not have, by calls (the original plays nothing).
    pub missing_clips: BTreeMap<String, u32>,
    /// Puppet callbacks that ran (a count for the tests).
    pub puppet_callbacks: u32,
    /// The C runtime's `rand`, which the characters' idle sway draws from.
    pub crt_rand: CrtRand,
    /// The animated objects' frame time in milliseconds (`manager +0x48`). **[I]** 24 Hz.
    pub frame_ms: f32,
    /// Events waiting for [`FrontEnd::pump`].
    pub queue: std::collections::VecDeque<FeEvent>,
    /// The string manager (`0xfb96b0`); without it text objects stay empty.
    pub strings: Option<Strings>,
    /// Every language's string table the data has (`language/<LANGUAGE>/COMMON.STS`), for `General_ChangeLanguage`.
    /// Empty: any language is taken and the table stays.
    pub languages: Vec<qwc_formats::sts::Sts>,
    /// Calls of natives not ported yet, by name (or hash).
    pub missing: BTreeMap<String, u32>,
    /// The host's textures and fonts as the natives see them (`GetCalculatedExtents`, the text layout when a string
    /// is set); without them texts are laid out as they draw.
    pub assets: Option<std::rc::Rc<dyn crate::draw::Assets>>,
    /// The wipe resources (type 5) by resource id, which `LayerObj_ActivateWipe` plays. **[I]** The original finds
    /// them in the loaded packages of the layer's screen (`0x4ea5b0`); the host gives them all.
    pub wipe_files: HashMap<u32, std::rc::Rc<crate::wipe::WipeData>>,
    /// Wipes that ended in the last draw: (script, callback, the layer's first top-level object).
    pub wipe_callbacks: Vec<(usize, u32, u32)>,
    /// Whether DirectInput opened the keyboard and the mouse (`[0xeb0aec]`, `[0xeb0aed]`, set by `0x508b60`): then
    /// players 0 and 1 have controllers. **[I]** the host's window has both (true from the start).
    pub keyboard_and_mouse: bool,
    /// The cards' numbers (`qwc_formats::exe::card_numbers`, from the exe; the host reads them).
    pub card_numbers: Option<qwc_formats::exe::CardNumbers>,
    /// The Game Boy Advance link's packet (`0x1060104`, 16 bytes) as `GBALink_SendQCardPacket` left it.
    pub gba_packet: [u8; 16],
    /// Not the original's: the remaster's changes to where objects are drawn, by (their layer's hash, their id)
    /// ([`crate::draw::Adjust`]; the host reads them from its layout files).
    pub adjust: HashMap<(u32, u32), crate::draw::Adjust>,
    /// Not the original's: the remaster's replacement for a text object's string, by (its layer's hash, its id),
    /// in UTF-16 with the layout's control codes. It is laid out and drawn in place of what the scripts set
    /// ([`FrontEnd::text`]); the scripts still hold and read their own string.
    pub text_over: HashMap<(u32, u32), Vec<u16>>,
    /// Not the original's: the remaster's replacement for a sprite's picture, by (its layer's hash, its id):
    /// a minted image resource ([`crate::draw::IMAGE_SCRIPT`]) drawn in place of its own, at the image's size.
    pub image_over: HashMap<(u32, u32), u32>,
    /// Not the original's: the sprites the remaster adds to each layer, by the layer's hash. They become real
    /// objects of the layer ([`FrontEnd::sync_additions`]), so the walk draws them and the [`crate::draw::Adjust`]s
    /// move them; the scripts never look for their ids.
    pub additions: HashMap<u32, Vec<SpriteAdd>>,
    /// Not the original's: how far past the 512 x 512 screen the window reaches on each side, across and down,
    /// in its units (0 on a 4:3 window); where the [`crate::draw::Pin`]s take the objects. The host sets it.
    pub margins: [f32; 2],
    names: HashMap<u32, &'static str>,
    all_names: HashMap<u32, String>,
}

impl Default for FrontEnd {
    fn default() -> Self {
        FrontEnd::new()
    }
}

/// A front-end event (`0x4e23f0` allocates them; the listeners `0x4da270` and `0x4da2d0` take them).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeEvent {
    /// Types 3 and 4: highlight or unhighlight the objects with this id.
    Highlight {
        id: u32,
    },
    Unhighlight {
        id: u32,
    },
    /// Type 1: buttons on a controller (`flags` 1 a release, 2 a press; `qwc_feck::pad`). `buttons` bits
    /// 0x10/0x20 move a vertical menu back/forward and 0x40/0x80 a horizontal one.
    Input {
        pad: u8,
        flags: u32,
        buttons: u32,
    },
    /// Types 2 and 3: choose and cancel.
    Choose {
        pad: u8,
        flags: u32,
    },
    Cancel {
        pad: u8,
        flags: u32,
    },
}

/// An action finished and its callback must run.
struct Callback {
    script: usize,
    hash: u32,
    object: u32,
}

impl FrontEnd {
    pub fn new() -> FrontEnd {
        FrontEnd {
            objects: Vec::new(),
            layers: Vec::new(),
            datastore: Vec::new(),
            load_sets: Vec::new(),
            log: Vec::new(),
            audio_events: Vec::new(),
            requests: Vec::new(),
            commands: Vec::new(),
            input_blocked: Vec::new(),
            paused: false,
            stadium_render: false,
            card_texture: None,
            scene: false,
            hoops: false,
            engine: Vec::new(),
            lights: Lights::default(),
            force_feedback: false,
            rand_seed: 1,
            language: 0,
            display_modes: Vec::new(),
            display_mode: 0,
            widescreen: false,
            puppet_hack: 0,
            exit_request: false,
            input_toggle: 1,
            pads: Default::default(),
            render_to_texture: true,
            keys: Keys::default(),
            memcard: MemCard::default(),
            particles: Default::default(),
            puppets: BTreeMap::new(),
            anim_sets: None,
            missing_clips: BTreeMap::new(),
            puppet_callbacks: 0,
            crt_rand: CrtRand::default(),
            frame_ms: 1000.0 / 24.0,
            queue: Default::default(),
            strings: None,
            languages: Vec::new(),
            missing: BTreeMap::new(),
            assets: None,
            wipe_files: HashMap::new(),
            wipe_callbacks: Vec::new(),
            keyboard_and_mouse: true,
            card_numbers: None,
            gba_packet: [0; 16],
            adjust: HashMap::new(),
            text_over: HashMap::new(),
            image_over: HashMap::new(),
            additions: HashMap::new(),
            margins: [0.0; 2],
            names: NATIVES.iter().map(|n| (name_hash(n), *n)).collect(),
            all_names: HashMap::new(),
        }
    }

    /// Learn the names of the natives the loaded scripts import (for `missing`).
    pub fn learn_names(&mut self, vm: &Vm) {
        for s in &vm.scripts {
            for n in s.import_names() {
                self.all_names.insert(name_hash(n), n.to_string());
            }
        }
    }

    /// The host's resource choices run again (`Assets::refresh_choices`): a record with choices takes the entry its
    /// script's function answers now (the House Cup's players their house's, by the DataStore's `HogwartsTeam`).
    pub fn refresh_choices(&mut self, vm: &mut Vm) {
        if let Some(a) = self.assets.clone() {
            a.refresh_choices(&mut |s, f| vm.execute(self, s, f, &[]).ok());
        }
    }

    /// Build the `_LayerData` of a loaded script into a new layer (`FUN_004d9760`); `bytes` is the script's file.
    pub fn add_layer(&mut self, vm: &mut Vm, script: usize, bytes: &[u8]) -> Result<usize> {
        let v = Vso::parse(bytes).map_err(|e| Error::Format(e.to_string()))?;
        let (tree, _) = v.layers().map_err(|e| Error::Format(e.to_string()))?;
        let l = self.layers.len();
        let (mut objects, mut order) = (std::mem::take(&mut self.objects), Vec::new());
        let mem = &mut vm.mem;
        let roots = layer::build(&tree, l, None, &mut objects, &mut order, &mut |w: &[u32]| {
            let a = mem.alloc(w.len() * 4);
            for (i, x) in w.iter().enumerate() {
                mem.set_u32(a + 4 * i as u32, *x).unwrap();
            }
            a
        });
        self.objects = objects;
        let (name, hash) = (vm.scripts[script].name.clone(), v.hash);
        for &i in &order {
            self.objects[i].script = hash;
        }
        let resources = v.resources().map_err(|e| Error::Format(e.to_string()))?;
        for &i in &order {
            if self.objects[i].kind == layer::Kind::Puppet {
                self.make_puppet(i, &resources);
            }
        }
        let wipe = Default::default();
        self.layers.push(Layer { script, name, hash, refs: 1, alive: true, screen: None, roots, objects: order, wipe });
        self.input_blocked.push(false);
        self.sync_layer_additions(l);
        Ok(l)
    }

    /// Bring every live layer's objects in line with [`FrontEnd::additions`]: added sprites made, moved or taken
    /// out. The host calls it after changing the additions; a layer built later takes them as it is built.
    pub fn sync_additions(&mut self) {
        for l in 0..self.layers.len() {
            if self.layers[l].alive {
                self.sync_layer_additions(l);
            }
        }
    }

    /// Layer `l`'s added sprites, as [`FrontEnd::additions`] has them now.
    fn sync_layer_additions(&mut self, l: usize) {
        use crate::draw::IMAGE_SCRIPT;
        let hash = self.layers[l].hash;
        let adds = self.additions.get(&hash).cloned().unwrap_or_default();
        // one added before and no longer wanted leaves the layer and its tree (the object itself stays, orphaned,
        // until the layer goes)
        let stale: Vec<usize> = (self.layers[l].objects.iter().copied())
            .filter(|&i| self.objects[i].script == IMAGE_SCRIPT && !adds.iter().any(|a| a.id == self.objects[i].id))
            .collect();
        for i in stale {
            if let Some(p) = self.objects[i].parent {
                self.objects[p].children.retain(|&c| c != i);
            }
            self.objects[i].parent = None;
            self.layers[l].roots.retain(|&r| r != i);
            self.layers[l].objects.retain(|&o| o != i);
        }
        for a in adds {
            let kind = if a.animate_like != 0 { layer::Kind::AnimatedSprite } else { layer::Kind::Sprite };
            // one whose kind changed is built again
            let found = (self.layers[l].objects.iter().copied()).find(|&i| {
                self.objects[i].script == IMAGE_SCRIPT && self.objects[i].id == a.id && self.objects[i].kind == kind
            });
            let i = match found {
                Some(i) => i,
                None => {
                    if let Some(old) = (self.layers[l].objects.iter().copied())
                        .find(|&i| self.objects[i].script == IMAGE_SCRIPT && self.objects[i].id == a.id)
                    {
                        if let Some(p) = self.objects[old].parent {
                            self.objects[p].children.retain(|&c| c != old);
                        }
                        self.objects[old].parent = None;
                        self.layers[l].roots.retain(|&r| r != old);
                        self.layers[l].objects.retain(|&o| o != old);
                    }
                    let mut o = layer::Object::new(kind, l);
                    o.script = IMAGE_SCRIPT;
                    o.id = a.id;
                    o.colour = [1.0; 4];
                    // an animated one copies its model's animation and starts it from the beginning; the model's
                    // live frame, program place and wait are its own
                    if let Some(m) = (a.animate_like != 0)
                        .then(|| {
                            (self.layers[l].objects.iter().copied()).find(|&k| {
                                self.objects[k].id == a.animate_like && self.objects[k].script != IMAGE_SCRIPT
                            })
                        })
                        .flatten()
                    {
                        let model = &self.objects[m];
                        o.program = model.program.clone();
                        for off in
                            [0xc4u16, 0xed, 0xee, 0xef, 0xf0, 0xf1, 0xf2, 0xf4, 0xf6, 0xf8, 0xfc, 0x108, 0x10c, 0x110]
                        {
                            o.set_slot(off, model.slot(off));
                        }
                    } else if kind == layer::Kind::AnimatedSprite {
                        // no model found: one frame, unit scales
                        o.set_slot(0xc4, 1);
                        o.set_slot(0x108, 1.0f32.to_bits());
                        o.set_slot(0x10c, 1.0f32.to_bits());
                    }
                    self.objects.push(o);
                    let i = self.objects.len() - 1;
                    self.layers[l].objects.push(i);
                    i
                }
            };
            // under the object of id `parent`, else a root, drawn after what is already there
            let want = (a.parent != 0)
                .then(|| (self.layers[l].objects.iter().copied()).find(|&k| k != i && self.objects[k].id == a.parent))
                .flatten();
            if self.objects[i].parent != want || (want.is_none() && !self.layers[l].roots.contains(&i)) {
                if let Some(p) = self.objects[i].parent {
                    self.objects[p].children.retain(|&c| c != i);
                }
                self.layers[l].roots.retain(|&r| r != i);
                match want {
                    Some(p) => {
                        self.objects[p].children.push(i);
                        self.objects[i].parent = Some(p);
                    }
                    None => {
                        self.layers[l].roots.push(i);
                        self.objects[i].parent = None;
                    }
                }
            }
            let o = &mut self.objects[i];
            o.pos = [a.pos[0], a.pos[1], 0.0, 0.0];
            o.layer_order = a.order;
            o.size = a.size;
            // a size of its own takes the sized flag; none, the image's
            if a.size == [0, 0] {
                o.flags &= !0x10000;
            } else {
                o.flags |= 0x10000;
            }
            if o.kind == layer::Kind::AnimatedSprite {
                // its resource is `+0xc0` (`+0xc8` is its frame); the material was the model's, else alpha
                o.set_slot(0xc0, a.image);
                if a.animate_like != 0 && o.slot(0xed) == 0 && o.program.is_empty() {
                    o.set_slot(0xed, 2);
                }
            } else {
                o.set_slot(0xbc, a.image);
                // alpha blended, as a PNG with transparency wants
                o.set_slot(0xc8, 2);
            }
        }
    }

    /// Run event `e`'s handler of every object in layer `l` that has one, in layer order (the dispatch loops
    /// `0x4d9840`), skipping disabled objects.
    pub fn dispatch(&mut self, vm: &mut Vm, l: usize, e: usize) -> Result<usize> {
        let script = self.layers[l].script;
        let mut ran = 0;
        for i in self.layers[l].objects.clone() {
            let o = &self.objects[i];
            if o.flags & flag::DISABLED != 0 || o.events[e] == 0 {
                continue;
            }
            let h = o.events[e];
            vm.execute(self, script, h, &[layer::object_handle(i)])?;
            ran += 1;
        }
        Ok(ran)
    }

    /// A general event (`OVNT` 11) with `value` for layer `l`: each enabled object's general handler runs with
    /// `[object, value]` (**[I]** the arguments; `HogwartsWelcome`'s `OnGeneral` reads the value second and leaves on
    /// 2, the cutscene's end).
    pub fn general(&mut self, vm: &mut Vm, l: usize, value: u32) -> Result<usize> {
        self.general_with(vm, l, &[value])
    }

    /// A general event with more arguments after its value (the handlers read them as their third and later
    /// arguments): the cutscenes' subtitles are event 5 with `(0, string id, shown)`.
    pub fn general_with(&mut self, vm: &mut Vm, l: usize, args: &[u32]) -> Result<usize> {
        let script = self.layers[l].script;
        let mut ran = 0;
        for i in self.layers[l].objects.clone() {
            let o = &self.objects[i];
            if o.flags & flag::DISABLED != 0 || o.events[layer::event::GENERAL] == 0 {
                continue;
            }
            let h = o.events[layer::event::GENERAL];
            let mut a = vec![layer::object_handle(i)];
            a.extend_from_slice(args);
            vm.execute(self, script, h, &a)?;
            ran += 1;
        }
        Ok(ran)
    }

    /// Run the callbacks of the wipes that ended in the last draw (`0x4ea8f0`: in the layer's script, with its first
    /// top-level object).
    pub fn run_wipe_callbacks(&mut self, vm: &mut Vm) -> Result<()> {
        for (script, hash, object) in std::mem::take(&mut self.wipe_callbacks) {
            vm.execute(self, script, hash, &[object])?;
        }
        Ok(())
    }

    /// One front-end frame of layer `l` (`0x4da880` per object): the process handler, then the actions, finishing
    /// those whose time is up and running their callbacks.
    pub fn process(&mut self, vm: &mut Vm, l: usize) -> Result<()> {
        let script = self.layers[l].script;
        let mut callbacks = Vec::new();
        for i in self.layers[l].objects.clone() {
            if self.objects[i].flags & 0x0a != 0 {
                continue;
            }
            let h = self.objects[i].events[event::PROCESS];
            if h != 0 {
                vm.execute(self, script, h, &[layer::object_handle(i)])?;
            }
            let mut k = 0;
            while k < self.objects[i].actions.len() {
                let mut a = self.objects[i].actions[k].clone();
                a.elapsed += 1;
                if a.elapsed < a.duration {
                    let t = a.elapsed as f32 / a.duration as f32;
                    update(&mut self.objects[i], &mut a, t, &vm.mem)?;
                    self.objects[i].actions[k] = a;
                    k += 1;
                } else if finish(&mut self.objects[i], &mut a) {
                    // a repeating timer starts again
                    a.elapsed = 0;
                    self.objects[i].actions[k] = a;
                    k += 1;
                } else {
                    self.objects[i].actions.remove(k);
                    if a.callback != 0 {
                        callbacks.push(Callback { script, hash: a.callback, object: layer::object_handle(i) });
                    }
                }
            }
            // the type's own step (`+0x1c`)
            if self.objects[i].kind == layer::Kind::AnimatedSprite {
                layer::animate(&mut self.objects[i]);
            }
        }
        for c in callbacks {
            vm.execute(self, c.script, c.hash, &[c.object])?;
        }
        Ok(())
    }

    /// The construct pass of a layer: every object's built-in construct (a menu picks its first choice,
    /// `0x4db7f0`), then its construct handler.
    /// **[I]** a menu whose start index a construct handler sets (`LayerObj_Menu_SetStartSelectionIndex`, as the
    /// Video options do with the current display mode) picks its choice again from it afterwards: the scripts set it
    /// there expecting the menu to start at it.
    pub fn construct(&mut self, vm: &mut Vm, l: usize) -> Result<usize> {
        let mut menus = Vec::new();
        for i in self.layers[l].objects.clone() {
            if self.objects[i].kind == layer::Kind::Menu && self.objects[i].flags & flag::DISABLED == 0 {
                self.menu_construct(&mut vm.mem, i)?;
                menus.push((i, self.objects[i].slot(0xc4)));
            }
        }
        let n = self.dispatch(vm, l, event::CONSTRUCT)?;
        for (i, start) in menus {
            if self.objects.get(i).is_some_and(|o| o.slot(0xc4) != start) {
                self.menu_construct(&mut vm.mem, i)?;
            }
        }
        Ok(n)
    }

    /// `0x4db7f0`: the start index (from the remembering DataStore entry when there is one), moved on to the
    /// first highlightable choice from there. That choice becomes the current one (set directly, the remembering
    /// entry untouched) and every child hears whether it is (`0x4db900`).
    fn menu_construct(&mut self, mem: &mut Memory, m: usize) -> Result<()> {
        let key = self.objects[m].slot(0xc8);
        if key != 0 {
            // the remembering entry (`+0xcc`): its first u16 is the start index
            if let Some(e) = self.entry(key) {
                self.objects[m].set_slot(0xcc, e.addr);
                self.objects[m].set_slot(0xc4, mem.u16(e.addr)? as u32);
            }
        }
        let mut start = self.objects[m].slot(0xc4) as u16 as i16;
        let mut current = None;
        for (n, &c) in self.objects[m].children.iter().enumerate() {
            if n as i16 == start {
                if self.objects[c].flags & flag::HIGHLIGHTABLE != 0 {
                    current = Some(c);
                    break;
                }
                start += 1;
            }
        }
        self.objects[m].set_slot(0xc4, start as u16 as u32);
        self.objects[m].set_slot(0xc0, current.map_or(0, layer::object_handle));
        if current.is_some() {
            self.child_update(m);
        }
        Ok(())
    }

    /// `0x4db900`: each child of menu `m` in turn hears a highlight (the current choice) or an unhighlight.
    /// **[I]** the original fails with `0x80030004` when its event pool runs out.
    fn child_update(&mut self, m: usize) {
        let cur = self.objects[m].slot(0xc0);
        for &c in &self.objects[m].children {
            let id = self.objects[c].id;
            self.queue.push_back(if layer::object_handle(c) == cur {
                FeEvent::Highlight { id }
            } else {
                FeEvent::Unhighlight { id }
            });
        }
    }

    /// `0x4dbb80`: make `new` the menu's choice, posting the unhighlight of the old one and the highlight of the
    /// new one.
    fn select(&mut self, mem: &mut Memory, m: usize, new: Option<usize>) -> Result<()> {
        let old = layer::object_index(self.objects[m].slot(0xc0));
        if old == new {
            return Ok(());
        }
        if let Some(o) = old {
            self.queue.push_back(FeEvent::Unhighlight { id: self.objects[o].id });
        }
        if let Some(n) = new {
            self.queue.push_back(FeEvent::Highlight { id: self.objects[n].id });
        }
        self.objects[m].set_slot(0xc0, new.map_or(0, layer::object_handle));
        // the remembering entry gets the new index
        let link = self.objects[m].slot(0xcc);
        if link != 0 {
            let n = new.and_then(|n| self.objects[m].children.iter().position(|&k| k == n));
            mem.set_u16(link, n.unwrap_or(self.objects[m].children.len()) as u16)?;
        }
        Ok(())
    }

    /// Run the queued events (the event manager's pump): highlights go to every object with that id that has a
    /// highlight or unhighlight handler (`0x4da270`); input goes to every object that listens for it
    /// (`0x4da2d0`): menus and objects with input, choose or cancel handlers. A handler that fails does not stop
    /// the delivery (`0x4da270` drops its result): every event still reaches every object, and the first error is
    /// returned at the end.
    /// `0x4d7700`: the pads' tick and both controllers' steps (`qwc_feck::pad`), their events queued.
    pub fn step_controllers(&mut self) {
        use crate::pad::Posted;
        for p in self.pads.step(&mut self.input_toggle) {
            self.queue.push_back(match p {
                Posted::Input { pad, flags, buttons } => FeEvent::Input { pad, flags, buttons },
                Posted::Choose { pad, flags } => FeEvent::Choose { pad, flags },
                Posted::Cancel { pad, flags } => FeEvent::Cancel { pad, flags },
            });
        }
    }

    pub fn pump(&mut self, vm: &mut Vm) -> Result<()> {
        let mut first = None;
        while let Some(ev) = self.queue.pop_front() {
            for l in 0..self.layers.len() {
                if !self.layers[l].alive {
                    continue;
                }
                let script = self.layers[l].script;
                for i in self.layers[l].objects.clone() {
                    if let Err(e) = self.deliver(vm, script, i, ev) {
                        first.get_or_insert(e);
                    }
                }
            }
        }
        first.map_or(Ok(()), Err)
    }

    fn deliver(&mut self, vm: &mut Vm, script: usize, i: usize, ev: FeEvent) -> Result<()> {
        let o = &self.objects[i];
        let h = layer::object_handle(i);
        if o.flags & flag::DISABLED != 0 {
            return Ok(());
        }
        match ev {
            FeEvent::Highlight { id } | FeEvent::Unhighlight { id } => {
                let listens = o.events[event::HIGHLIGHT] != 0 || o.events[event::UNHIGHLIGHT] != 0;
                if !listens || o.id != id || o.flags & flag::HIGHLIGHTABLE == 0 {
                    return Ok(());
                }
                // `0x4da510` / `0x4da590`
                let (set, e) = match ev {
                    FeEvent::Highlight { .. } => (true, event::HIGHLIGHT),
                    _ => (false, event::UNHIGHLIGHT),
                };
                let o = &mut self.objects[i];
                if set {
                    o.flags |= flag::HIGHLIGHTED;
                } else {
                    o.flags &= !flag::HIGHLIGHTED;
                }
                let f = o.events[e];
                if f != 0 {
                    vm.execute(self, script, f, &[h])?;
                }
            }
            FeEvent::Input { pad, flags, .. } | FeEvent::Choose { pad, flags } | FeEvent::Cancel { pad, flags } => {
                let listens = o.kind == layer::Kind::Menu
                    || [event::CHOOSE, event::CANCEL, event::INPUT].iter().any(|&e| o.events[e] != 0);
                let fl = o.flags;
                // `0x4da2d0`: not disabled (or flag 1), the layer takes input, highlighted or listening on a pad
                if !listens || fl & 0x09 != 0 || self.input_blocked[o.layer] || fl & 0x1fa0 == 0 {
                    return Ok(());
                }
                let on_pad = fl & flag::INPUT_ANY != 0 && fl & (0x80 << pad) != 0;
                let (kind, buttons) = match ev {
                    FeEvent::Input { buttons, .. } => (1u32, buttons),
                    FeEvent::Choose { .. } => (2, 0),
                    _ => (3, 0),
                };
                if kind == 1 && on_pad {
                    // `0x4da6a0`: the object's own input (a menu moves its choice), then its input handler
                    if self.objects[i].kind == layer::Kind::Menu {
                        self.menu_input(vm, script, i, flags, buttons)?;
                    }
                    let f = self.objects[i].events[event::INPUT];
                    if f != 0 {
                        vm.execute(self, script, f, &[h, kind, pad as u32, flags, buttons])?;
                    }
                } else if kind != 1 && flags & 1 != 0 && on_pad {
                    // `0x4da740` / `0x4da7e0`
                    let e = if kind == 2 { event::CHOOSE } else { event::CANCEL };
                    let f = self.objects[i].events[e];
                    if f != 0 {
                        vm.execute(self, script, f, &[h, kind, pad as u32, flags, 0])?;
                    }
                }
            }
        }
        Ok(())
    }

    /// A menu's input (`0x4db890`): back and forward over its highlightable choices.
    fn menu_input(&mut self, vm: &mut Vm, script: usize, m: usize, flags: u32, buttons: u32) -> Result<()> {
        let mf = self.objects[m].slot(0xbc);
        if (mf & 2 != 0 && flags & 1 == 0) || flags & 2 == 0 || self.objects[m].slot(0xc0) == 0 {
            return Ok(());
        }
        let (back, fwd) = if mf & 1 == 0 { (0x10, 0x20) } else { (0x40, 0x80) };
        if buttons & back != 0 {
            self.menu_step(vm, script, m, false)
        } else if buttons & fwd != 0 {
            self.menu_step(vm, script, m, true)
        } else {
            Ok(())
        }
    }

    /// `0x4dba00` (forward) and `0x4dbaa0` (back): the next highlightable choice that way, wrapping when the
    /// menu's flag 4 is set; the moved handler (`+0xdc` forward, `+0xd8` back) or the wrap handler (`+0xd4`,
    /// `+0xd0`) runs with `(menu, menu, old, new)`.
    fn menu_step(&mut self, vm: &mut Vm, script: usize, m: usize, forward: bool) -> Result<()> {
        let kids = self.objects[m].children.clone();
        let cur = layer::object_index(self.objects[m].slot(0xc0));
        let Some(at) = cur.and_then(|c| kids.iter().position(|&k| k == c)) else { return Ok(()) };
        let ok = |fe: &FrontEnd, k: usize| fe.objects[k].flags & flag::HIGHLIGHTABLE != 0;
        let wrap = self.objects[m].slot(0xbc) & 4 != 0;
        let (before, after) = (&kids[..at], &kids[at + 1..]);
        let mut order: Vec<usize> = Vec::new();
        if forward {
            order.extend(after);
            if wrap {
                order.extend(before);
            }
        } else {
            order.extend(before.iter().rev());
            if wrap {
                order.extend(after.iter().rev());
            }
        }
        let Some(&next) = order.iter().find(|&&k| ok(self, k)) else { return Ok(()) };
        let pos = kids.iter().position(|&k| k == next).unwrap();
        let wrapped = if forward { pos < at } else { pos > at };
        self.select(&mut vm.mem, m, Some(next))?;
        let handler = self.objects[m].slot(match (forward, wrapped) {
            (true, false) => 0xdc,
            (true, true) => 0xd4,
            (false, false) => 0xd8,
            (false, true) => 0xd0,
        });
        if handler != 0 {
            let (mh, old, new) = (layer::object_handle(m), layer::object_handle(kids[at]), layer::object_handle(next));
            vm.execute(self, script, handler, &[mh, mh, old, new])?;
        }
        Ok(())
    }

    fn obj(&mut self, h: u32) -> Result<&mut Object> {
        let i = layer::object_index(h).ok_or(Error::Native(format!("{h:#x} is not an object")))?;
        self.objects.get_mut(i).ok_or(Error::Native(format!("no object {i}")))
    }

    fn entry(&self, key: u32) -> Option<Entry> {
        self.datastore.iter().find(|e| e.key == key).copied()
    }

    /// `General_TotalQuidditchCards` (`0x4d4ec0`): count the card bits (row `i` of `KEY_CARD_BITS` holds
    /// `CARDS_PER_ROW[i]`), correct the stored total when it differs, and with 103 cards award the last one (bit 3
    /// of row 4, written into the card bits and the unlock bits alike) for 104.
    pub fn total_quidditch_cards(&mut self, mem: &mut Memory) -> Result<u32> {
        let bits = self.entry(KEY_CARD_BITS);
        let row = |mem: &Memory, i: u32| bits.and_then(|e| mem.u16(e.addr + e.stride as u32 * i).ok()).unwrap_or(0);
        let mut n = 0;
        for (i, &cards) in CARDS_PER_ROW.iter().enumerate() {
            let v = row(mem, i as u32) as u32;
            n += (0..cards).filter(|b| v & 1 << b != 0).count() as u32;
        }
        let total = self.datastore_u32(mem, KEY_CARD_TOTAL).unwrap_or(0);
        if total != n {
            self.log.push(format!("***Quidditch card total mismatch: TotalNoOfQuidditchCards = {total}, actual = {n}"));
            self.set_datastore_u32(mem, KEY_CARD_TOTAL, n);
        }
        if n != 103 {
            return Ok(n);
        }
        self.log.push("Quidditch cards - 103 awarded, awarding final card".to_string());
        let v = row(mem, 4) | 8;
        for key in [KEY_CARD_BITS, KEY_UNLOCK_BITS] {
            if let Some(e) = self.entry(key) {
                mem.set_u16(e.addr + e.stride as u32 * 4, v)?;
            }
        }
        self.set_datastore_u32(mem, KEY_CARD_TOTAL, 104);
        Ok(104)
    }

    /// `GBALink_SendQCardPacket(_, kind, a, b)` (`0x4d4cb0`): a packet for the Game Boy Advance into
    /// [`FrontEnd::gba_packet`]. Nothing in the exe sends it or reads it again: on PC the link is never connected
    /// (`GBALink_IsConnected` is the shared `return 0`).
    /// - `0x55` (`0x4d4ab0`): the owned cards, the whole 16 bytes cleared, then for each of 15 rows of the card bits
    ///   (`0xacfe6233`, a missing entry 0) each of its 10 bits set stands for a card numbered by the exe's table; card
    ///   `n` sets bit `(n - 1) % 8` of byte `1 + (n - 1) / 8`.
    /// - `0x56`: the number of card `b` of row `a`, less 1; `0x57`: `a`'s low byte; `0x67` alone. These clear the
    ///   first 4 bytes only.
    /// - Any other kind changes nothing. **[I]** without the table, `0x55` holds no cards and `0x56` -1.
    pub fn gba_packet(&mut self, mem: &Memory, kind: u32, a: u32, b: u32) {
        let number = |row: i64, k: i64| {
            let t = self.card_numbers?;
            let at = row.checked_mul(10)?.checked_add(k)?;
            t.as_flattened().get(usize::try_from(at).ok()?).copied()
        };
        let head = |p: &mut [u8; 16], bytes: [u8; 2]| {
            p[..4].copy_from_slice(&[bytes[0], bytes[1], 0, 0]);
        };
        match kind {
            0x55 => {
                let bits = self.entry(KEY_CARD_BITS);
                let mut p = [0u8; 16];
                p[0] = 0x55;
                for row in 0..15 {
                    let v = bits.and_then(|e| mem.u16(e.addr + e.stride as u32 * row).ok()).unwrap_or(0);
                    for k in (0..10).filter(|k| v & 1 << k != 0) {
                        let card = number(row as i64, k).unwrap_or(-1) as i32 - 1;
                        if card >= 0 {
                            p[1 + card as usize / 8] |= 1 << (card % 8);
                        }
                    }
                }
                self.gba_packet = p;
            }
            0x56 => {
                let card = number(a as i32 as i64, b as i32 as i64).unwrap_or(0).wrapping_sub(1);
                head(&mut self.gba_packet, [0x56, card as u8]);
            }
            0x57 => head(&mut self.gba_packet, [0x57, a as u8]),
            0x67 => head(&mut self.gba_packet, [0x67, 0]),
            _ => {}
        }
    }

    /// A DataStore entry's first element as a `u32` (the `@Key` load sets read it this way, `0x4fd6f0`).
    pub fn datastore_u32(&self, mem: &Memory, key: u32) -> Option<u32> {
        mem.u32(self.entry(key)?.addr).ok()
    }

    /// Element `i` of a DataStore entry of 16-bit elements (or the low half of wider ones), if there is one.
    pub fn datastore_u16(&self, mem: &Memory, key: u32, i: u32) -> Option<u16> {
        let e = self.entry(key)?;
        (i < u32::from(e.count.max(1))).then_some(())?;
        mem.u16(e.addr + u32::from(e.stride) * i).ok()
    }

    /// Set a DataStore entry's first byte, creating a `U8` entry for a missing key.
    pub fn set_datastore_u8(&mut self, mem: &mut Memory, key: u32, v: u8) {
        if self.entry(key).is_none() {
            let addr = mem.alloc(4);
            self.datastore.push(Entry { key, kind: 0, count: 1, stride: 1, addr });
        }
        if let Some(e) = self.entry(key) {
            let _ = mem.set_u8(e.addr, v);
        }
    }

    /// Set a DataStore entry's first element; a missing key is ignored.
    pub fn set_datastore_u32(&self, mem: &mut Memory, key: u32, v: u32) {
        if let Some(e) = self.entry(key) {
            let _ = mem.set_u32(e.addr, v);
        }
    }

    /// A text object's string again (`0x4dd6d0`, run by the text natives and when a text is first drawn): with a
    /// string id set (`+0xbc`), the handle it holds (`+0xc4`, stored here as handle + 1) is released and string
    /// `id + index` (`+0xe8`) acquired, formatted from the DataStore. A failed acquire leaves no handle. Then the
    /// text is laid out (`0x4dd8c0`), which here only keeps its width and height (`+0xd4`, `+0xd6`, its extent) when
    /// the host's [`FrontEnd::assets`] have its font; the lines are laid out again as it draws.
    pub fn reformat(&mut self, mem: &Memory, i: usize) {
        let o = &self.objects[i];
        let (id, held, index) = (o.slot(0xbc), o.slot(0xc4), o.slot(0xe8));
        let Some(strings) = self.strings.as_mut() else {
            return;
        };
        if id == 0 {
            return;
        }
        if held != 0 {
            strings.release(held as usize - 1);
            self.objects[i].set_slot(0xc4, 0);
        }
        let view = DataStoreView { entries: &self.datastore, mem };
        if let Ok(Some(h)) = strings.acquire(id, index, &view) {
            self.objects[i].set_slot(0xc4, h as u32 + 1);
        }
        let Some(assets) = self.assets.clone() else { return };
        let Some(font) = assets.font(self.objects[i].slot(0xc0)) else { return };
        let l = self.objects[i].layer;
        if let Some(lay) = self.layout_text(i, font, &|id| self.inline_extent(l, id, &*assets)) {
            self.objects[i].set_slot(0xd4, lay.width as u32);
            self.objects[i].set_slot(0xd6, lay.height as u32);
        }
    }

    /// Release the strings layer `l`'s text objects hold (`+0xc4`, `0x4e0b20`), as the objects' destruction does:
    /// a string's formatted text lives only while someone holds it.
    pub fn release_texts(&mut self, l: usize) {
        let Some(strings) = self.strings.as_mut() else {
            return;
        };
        for &i in &self.layers[l].objects {
            let o = &mut self.objects[i];
            if o.kind == layer::Kind::Text {
                let held = o.slot(0xc4);
                if held != 0 {
                    strings.release(held as usize - 1);
                    o.set_slot(0xc4, 0);
                }
            }
        }
    }

    /// Format every text object of layer `l` that holds no string yet (the draw path `0x4dd7a0` does this before
    /// drawing).
    pub fn format_texts(&mut self, mem: &Memory, l: usize) {
        for i in self.layers[l].objects.clone() {
            if self.objects[i].kind == layer::Kind::Text && self.objects[i].slot(0xc4) == 0 {
                self.reformat(mem, i);
            }
        }
    }

    /// Lay out text object `i`'s string (`0x4dd8c0`) in `font`; inline sprites are measured by `extent`. `None`
    /// when it holds no string.
    pub fn layout_text(
        &self,
        i: usize,
        font: &dyn Metrics,
        extent: &dyn Fn(u32) -> Option<(f32, f32)>,
    ) -> Option<Layout> {
        let text = self.text(i)?;
        let o = &self.objects[i];
        // the width of the first ancestor with flag 0x30000, else 512
        let mut avail = 0x200;
        let mut a = o.parent;
        while let Some(k) = a {
            if self.objects[k].flags & 0x30000 != 0 {
                avail = self.objects[k].size[0];
                break;
            }
            a = self.objects[k].parent;
        }
        let p = Params { effect: o.slot(0xc8), avail, x: o.pos[0], y: o.pos[1], tabs: o.tabs };
        Some(layout(text, font, extent, p))
    }

    /// Switch the strings to the table of language `hash` (`Strings::switch`); false when the data has none.
    fn switch_language(&mut self, mem: &Memory, hash: u32) -> bool {
        let Some(sts) = self.languages.iter().find(|s| s.language == hash).cloned() else {
            return false;
        };
        let view = DataStoreView { entries: &self.datastore, mem };
        if let Some(strings) = self.strings.as_mut() {
            strings.switch(sts, &view);
        }
        true
    }

    /// `0x52e170`'s first part: the DataStore's flags for the sizes the Video options gate
    /// (`TeamTentOptionsVideo`'s `OnScreenResolutionConstruct` shows an entry only when its flag is set), each 1
    /// when [`FrontEnd::display_modes`] has that size. **[I]** the rest of `0x52e170` (two settings from
    /// `0x5e69f0`, `0x5e69f4` into `0xfe53e9a2`, `0xfbed26f4`) is not ported.
    pub fn note_display_modes(&mut self, mem: &mut Memory) {
        const SIZES: [((u32, u32), u32); 4] = [
            ((640, 480), 0x49d6_243d),
            ((800, 600), 0x4925_4310),
            ((1024, 768), 0xd2a6_1cdc),
            ((1152, 864), 0xe47e_d966),
        ];
        for (size, key) in SIZES {
            let has = self.display_modes.iter().any(|&(w, h, _)| (w, h) == size);
            if has {
                self.set_datastore_u32(mem, key, 1);
            }
        }
    }

    /// Start in language `hash` (the host's choice, before the screens choose their fonts and logo): the strings
    /// switched as `General_ChangeLanguage` does, and the DataStore's `crcCurrentLanguage`, which the scripts read,
    /// set to it. False, and nothing changed, when the data has no table for it.
    pub fn set_language(&mut self, mem: &mut Memory, hash: u32) -> bool {
        if !self.switch_language(mem, hash) {
            return false;
        }
        self.language = hash;
        self.set_datastore_u32(mem, crate::text::CURRENT_LANGUAGE, hash);
        true
    }

    /// The DataStore as the string formatters read it.
    pub fn values<'a>(&'a self, mem: &'a Memory) -> impl Values + 'a {
        DataStoreView { entries: &self.datastore, mem }
    }

    /// The text a text object shows (UTF-16 with the layout's control codes): the remaster's replacement when one
    /// is set ([`FrontEnd::text_over`], even where the scripts gave it no string), else the scripts' own.
    pub fn text(&self, i: usize) -> Option<&[u16]> {
        let o = self.objects.get(i)?;
        if let Some(t) = self.text_over.get(&(self.layers[o.layer].hash, o.id)) {
            return Some(t);
        }
        self.script_text(i)
    }

    /// The text the scripts gave a text object, replaced or not, when it holds a formatted string.
    pub fn script_text(&self, i: usize) -> Option<&[u16]> {
        let h = self.objects.get(i)?.slot(0xc4);
        self.strings.as_ref()?.text((h as usize).checked_sub(1)?)
    }

    fn add_action(&mut self, h: u32, kind: ActionKind, id: u32, to: [f32; 4], ms: u32, callback: u32) -> Result<u32> {
        let o = self.obj(h)?;
        let from = match kind {
            ActionKind::Alpha => [o.colour[3], 0.0, 0.0, 0.0],
            ActionKind::Colour => o.colour,
            ActionKind::Position => o.pos,
            ActionKind::Size => [o.size[0] as f32, o.size[1] as f32, 0.0, 0.0],
            ActionKind::Timer | ActionKind::Spline { .. } | ActionKind::Animate { .. } => [0.0; 4],
        };
        if let ActionKind::Spline { spline: s, .. } = kind {
            spline::begin(o, s);
        }
        if kind == ActionKind::Size {
            o.flags |= 0x10000;
        }
        o.actions.push(Action { kind, id, elapsed: 0, duration: frames(ms), from, to, callback });
        Ok(0)
    }
}

/// `trunc(ms * 0.024)`: milliseconds to 24 Hz frames.
pub fn frames(ms: u32) -> u32 {
    (ms as i32 as f32 * 0.024).trunc() as u32
}

/// The actions' update methods (`0x4d5170`, `0x4d51d0`, `0x4d5260`, the size update; the timer's does nothing).
/// The splines' are in `qwc_feck::spline`.
fn update(o: &mut Object, a: &mut Action, t: f32, mem: &Memory) -> Result<()> {
    if let ActionKind::Animate { .. } = a.kind {
        animate(o, &mut a.kind, t);
        return Ok(());
    }
    let lerp = |k: usize| (a.to[k] - a.from[k]) * t + a.from[k];
    match a.kind {
        ActionKind::Alpha => o.colour[3] = (a.to[0] - a.from[0]) * t + a.from[0],
        ActionKind::Colour => o.colour = [lerp(0), lerp(1), lerp(2), o.colour[3]],
        ActionKind::Position => o.pos = [lerp(0), lerp(1), lerp(2), 0.0],
        ActionKind::Size => o.size = [lerp(0).trunc() as u16, lerp(1).trunc() as u16],
        ActionKind::Timer => {}
        ActionKind::Spline { spline: s, keys, count, looping } => {
            // **[I]** a spline whose keys cannot be read ends (the challenge select's `MoveCamera` passes its
            // uninitialised locals for a move between choices that are not neighbours)
            if spline::update(o, s, mem, keys, count, looping, &mut a.elapsed, a.duration, t).is_err() {
                a.elapsed = a.duration;
            }
        }
        ActionKind::Animate { .. } => {}
    }
    Ok(())
}

/// `0x4d5320`, the animate action's update: frame `count × t` (backwards `(1 - t) × count`), truncated, counted
/// from the frame bound as it began (`0x4d52f0`, tried again each update until the sprite has drawn), shown when
/// it is not the last one shown. The original binds the frame's texture there (`0x4dd3c0`, centring with
/// `+0xd8 & 2`); the port notes the frame and the sprite's draw binds it.
fn animate(o: &mut Object, kind: &mut ActionKind, t: f32) {
    let ActionKind::Animate { count, dir, base, last, .. } = kind else { return };
    let k = if *dir > 0 { layer::ftol(*count as f32 * t) } else { layer::ftol((1.0 - t) * *count as f32) };
    if base.is_none() {
        *base = crate::draw::bound_frame(o);
    }
    if let Some(b) = *base {
        if k != *last {
            crate::draw::set_sprite_frame(o, b.wrapping_add(k as u8));
            *last = k;
        }
    }
}

/// The finish methods (`0x4d5190`, `0x4d5210`, `0x4d52b0`, `0x4d5470`, `0x4d52d0`): set the end value; a timer
/// counts down its repeats and asks to run again while any are left (a count of 0 repeats for ever). The animate
/// action (`0x4d5380`) turns round and runs again when it goes back and forth, else ends on its last frame.
fn finish(o: &mut Object, a: &mut Action) -> bool {
    if let ActionKind::Animate { ping_pong, ref mut dir, .. } = a.kind {
        if ping_pong {
            *dir = -*dir;
            return true;
        }
        animate(o, &mut a.kind, 1.0);
        return false;
    }
    match a.kind {
        ActionKind::Alpha => o.colour[3] = a.to[0],
        ActionKind::Colour => o.colour = [a.to[0], a.to[1], a.to[2], o.colour[3]],
        ActionKind::Position => o.pos = [a.to[0], a.to[1], a.to[2], 0.0],
        ActionKind::Size => o.size = [a.to[0] as u16, a.to[1] as u16],
        ActionKind::Timer => {
            a.from[0] -= 1.0;
            return a.from[0] != 0.0;
        }
        ActionKind::Spline { spline: s, .. } => spline::finish(o, s),
        ActionKind::Animate { .. } => {}
    }
    false
}

/// `FECK_Printf` with the slots as its arguments (`%d %i %u %x %X %c %s %f %%`; a float takes one slot).
fn printf(mem: &Memory, fmt: &[u8], args: &[u32]) -> String {
    let (mut out, mut a, mut i) = (String::new(), args.iter(), 0);
    while i < fmt.len() {
        let c = fmt[i];
        i += 1;
        if c != b'%' {
            out.push(c as char);
            continue;
        }
        // skip flags, width, precision and length
        while i < fmt.len() && b"-+ #0123456789.lh".contains(&fmt[i]) {
            i += 1;
        }
        let Some(&conv) = fmt.get(i) else { break };
        i += 1;
        let v = if conv == b'%' { 0 } else { a.next().copied().unwrap_or(0) };
        match conv {
            b'd' | b'i' => out += &(v as i32).to_string(),
            b'u' => out += &v.to_string(),
            b'x' => out += &format!("{v:x}"),
            b'X' => out += &format!("{v:X}"),
            b'c' => out.push(v as u8 as char),
            b's' => out += &String::from_utf8_lossy(&mem.cstr(v).unwrap_or_default()),
            b'f' => out += &format!("{:.6}", f32::from_bits(v)),
            b'g' | b'e' => out += &f32::from_bits(v).to_string(),
            b'%' => out.push('%'),
            other => out.push(other as char),
        }
    }
    out
}

/// The DataStore as the string formatters read it: each entry's `count * stride` bytes from VM memory.
struct DataStoreView<'a> {
    entries: &'a [Entry],
    mem: &'a Memory,
}

impl Values for DataStoreView<'_> {
    fn value(&self, key: u32) -> Option<Value> {
        let e = self.entries.iter().find(|e| e.key == key)?;
        let len = e.count as u32 * e.stride as u32;
        let data = self.mem.read(e.addr, len).ok()?.to_vec();
        Some(Value { kind: e.kind, count: e.count, stride: e.stride, data })
    }
}

impl Host for FrontEnd {
    fn has_native(&self, hash: u32) -> bool {
        // every native of the exe binds; the ones not ported answer 0 and are counted
        EXE_NATIVES.contains(&hash)
    }

    fn call(&mut self, vm: &mut Vm, hash: u32, args: &[u32], _floats: u32) -> Result<u32> {
        let arg = |i: usize| args.get(i).copied().unwrap_or(0);
        let f = |i: usize| f32::from_bits(arg(i));
        let Some(name) = self.names.get(&hash).copied() else {
            let n = self.all_names.get(&hash).cloned().unwrap_or(format!("{hash:#010x}"));
            *self.missing.entry(n).or_default() += 1;
            return Ok(0);
        };
        if name.starts_with("Memcard_") && std::env::var_os("QWC_MC_LOG").is_some() {
            eprintln!("{name}{args:x?}");
        }
        let mem = &mut vm.mem;
        // a null object answers 0 (the scripts test their results)
        let objects = |fe: &FrontEnd, h: u32| layer::object_index(h).filter(|&i| i < fe.objects.len());
        // a script handler the native runs as its last step (`0x4dbc20`), once `mem` is free again
        let mut then: Option<(usize, u32, Vec<u32>)> = None;
        // a native's error names it
        let mut run = || -> Result<u32> {
            Ok(match name {
                "LayerObj_SetX" | "LayerObj_SetY" | "LayerObj_SetZ" => {
                    let k = (name.as_bytes()[12] - b'X') as usize;
                    self.obj(arg(0))?.pos[k] = f(1);
                    0
                }
                "LayerObj_SetW" => {
                    self.obj(arg(0))?.size[0] = f(1).trunc() as i32 as u16;
                    0
                }
                "LayerObj_SetH" => {
                    self.obj(arg(0))?.size[1] = f(1).trunc() as i32 as u16;
                    0
                }
                "LayerObj_GetX" | "LayerObj_GetY" | "LayerObj_GetZ" => {
                    let k = (name.as_bytes()[12] - b'X') as usize;
                    objects(self, arg(0)).map_or(0, |i| self.objects[i].pos[k].to_bits())
                }
                // `0x4d5530`, `0x4d5540`: the 16-bit sizes as integers
                "LayerObj_GetW" => objects(self, arg(0)).map_or(0, |i| self.objects[i].size[0] as u32),
                "LayerObj_GetH" => objects(self, arg(0)).map_or(0, |i| self.objects[i].size[1] as u32),
                "LayerObj_GetFlags" => objects(self, arg(0)).map_or(0, |i| self.objects[i].flags),
                "LayerObj_SetFlag" | "LayerObj_ResetFlag" => {
                    // `0x4d55c0` / `0x4d5600`, down the children when asked
                    if let Some(i) = objects(self, arg(0)) {
                        let mut stack = vec![i];
                        while let Some(k) = stack.pop() {
                            let o = &mut self.objects[k];
                            if name == "LayerObj_SetFlag" {
                                o.flags |= arg(1);
                            } else {
                                o.flags &= !arg(1);
                            }
                            if arg(2) as u8 != 0 {
                                stack.extend(o.children.iter().copied());
                            }
                        }
                    }
                    0
                }
                "LayerObj_GetObjectID" => objects(self, arg(0)).map_or(0, |i| self.objects[i].id),
                "LayerObj_SetPosition" => {
                    self.obj(arg(0))?.pos = [f(1), f(2), f(3), 0.0];
                    0
                }
                "LayerObj_SetRGBA" => {
                    self.obj(arg(0))?.colour = [f(1), f(2), f(3), f(4)];
                    0
                }
                "LayerObj_SetR" | "LayerObj_SetG" | "LayerObj_SetB" | "LayerObj_SetA" => {
                    let k = b"RGBA".iter().position(|&c| c == name.as_bytes()[12]).unwrap();
                    self.obj(arg(0))?.colour[k] = f(1);
                    0
                }
                "LayerObj_GetRotX" | "LayerObj_GetRotY" | "LayerObj_GetRotZ" => {
                    let k = (name.as_bytes()[15] - b'X') as usize;
                    objects(self, arg(0)).map_or(0, |i| self.objects[i].rot[k].to_bits())
                }
                "LayerObj_SetRot" => {
                    self.obj(arg(0))?.rot = [f(1), f(2), f(3)];
                    0
                }
                "LayerObj_SetRotX" | "LayerObj_SetRotY" | "LayerObj_SetRotZ" => {
                    let k = (name.as_bytes()[15] - b'X') as usize;
                    self.obj(arg(0))?.rot[k] = f(1);
                    0
                }
                "LayerObj_GetObject" => objects(self, arg(0)).map_or(0, |i| {
                    // `0x4d5780`: every object of the object's layer, by id
                    let l = self.objects[i].layer;
                    self.layers[l]
                        .objects
                        .iter()
                        .find(|&&k| self.objects[k].id == arg(1))
                        .map_or(0, |&k| layer::object_handle(k))
                }),
                "LayerObj_GetChildObject" => objects(self, arg(0)).map_or(0, |i| {
                    // the n-th child; the count is a signed 16-bit value
                    let n = arg(1) as i16;
                    if n < 0 {
                        0
                    } else {
                        self.objects[i].children.get(n as usize).map_or(0, |&k| layer::object_handle(k))
                    }
                }),
                "LayerObj_GetSiblingObject" => objects(self, arg(0)).map_or(0, |i| {
                    let siblings = match self.objects[i].parent {
                        Some(p) => &self.objects[p].children,
                        None => &self.layers[self.objects[i].layer].roots,
                    };
                    let at = siblings.iter().position(|&k| k == i).unwrap();
                    siblings.get(at + 1).map_or(0, |&k| layer::object_handle(k))
                }),
                "LayerObj_GetParentObject" => {
                    objects(self, arg(0)).and_then(|i| self.objects[i].parent).map_or(0, layer::object_handle)
                }
                "LayerObj_GetChildCount" => objects(self, arg(0)).map_or(0, |i| self.objects[i].children.len() as u32),
                "LayerObj_GetUserParam" => {
                    // `0x4d5810`: the block's address, its size in bytes to `*arg(2)` when given
                    let found = objects(self, arg(0)).and_then(|i| self.objects[i].user.iter().find(|u| u.0 == arg(1)));
                    let (addr, size) = found.map_or((0, 0), |u| (u.1, u.2));
                    if arg(2) != 0 {
                        mem.set_u32(arg(2), size)?;
                    }
                    addr
                }
                "LayerObj_KillAction" => {
                    // `0x4d5940`: the action ends at its final value, without its callback
                    if let Some(i) = objects(self, arg(0)) {
                        if let Some(k) = self.objects[i].actions.iter().position(|a| a.id == arg(1)) {
                            let mut a = self.objects[i].actions.remove(k);
                            finish(&mut self.objects[i], &mut a);
                        }
                    }
                    0
                }
                "LayerObj_AddAction_Alpha" => {
                    self.add_action(arg(0), ActionKind::Alpha, arg(1), [f(2), 0.0, 0.0, 0.0], arg(3), arg(4))?
                }
                "LayerObj_AddAction_Colour" => {
                    self.add_action(arg(0), ActionKind::Colour, arg(1), [f(2), f(3), f(4), 0.0], arg(5), arg(6))?
                }
                "LayerObj_AddAction_Position" => {
                    self.add_action(arg(0), ActionKind::Position, arg(1), [f(2), f(3), f(4), 0.0], arg(5), arg(6))?
                }
                "LayerObj_AddAction_Timer" => {
                    let count = arg(2) as i32 as f32;
                    self.add_action(arg(0), ActionKind::Timer, arg(1), [0.0; 4], arg(3), arg(4))?;
                    let o = self.obj(arg(0))?;
                    o.actions.last_mut().unwrap().from[0] = count;
                    0
                }
                "LayerObj_AddAction_Size" => {
                    let to = [arg(2) as u16 as f32, arg(3) as u16 as f32, 0.0, 0.0];
                    self.add_action(arg(0), ActionKind::Size, arg(1), to, arg(4), arg(5))?
                }
                // `0x4d6200` (object, id, frames, backwards, back and forth, ms, callback)
                "LayerObj_Sprite_AddAction_Animate" => {
                    let kind = ActionKind::Animate {
                        count: arg(2),
                        dir: if arg(3) as u8 == 0 { 1 } else { -1 },
                        ping_pong: arg(4) as u8 != 0,
                        base: None,
                        last: -999,
                    };
                    self.add_action(arg(0), kind, arg(1), [0.0; 4], arg(5), arg(6))?
                }
                // `0x4d5550` (object, &width, &height): the object measured (`0x4da340`) and its extent's width and
                // height truncated; the result is the height as the native leaves it. **[I]** The original measures
                // from whatever its stack holds as the parent's place, which only the places stored depend on; the
                // port from 0
                "LayerObj_GetCalculatedExtents" => {
                    let i = objects(self, arg(0)).ok_or(Error::Native("GetCalculatedExtents: no object".into()))?;
                    let assets = self.assets.clone();
                    let ext = match &assets {
                        Some(a) => self.measure(i, [0.0; 4], &**a),
                        None => self.measure(i, [0.0; 4], &crate::draw::NoAssets),
                    };
                    let (w, h) = (layer::ftol(ext[0]) as u32, layer::ftol(ext[1]) as u32);
                    mem.set_u32(arg(1), w)?;
                    mem.set_u32(arg(2), h)?;
                    h
                }
                // `0x4d5df0` (object, resource, backwards, ms, callback): a wipe on the object's layer; 1 without the
                // resource
                "LayerObj_ActivateWipe" => {
                    let l = self.obj(arg(0))?.layer;
                    match self.wipe_files.get(&arg(1)).cloned() {
                        Some(data) => {
                            let w = &mut self.layers[l].wipe;
                            w.data = Some(data);
                            w.reverse = arg(2) as u8 != 0;
                            w.frames = frames(arg(3));
                            w.callback = arg(4);
                            w.elapsed = 0;
                            0
                        }
                        None => 1,
                    }
                }
                // `0x4d5e60` (object, id, value, test, once): the test is the sign of its argument (`0x4ea960`)
                "LayerObj_AddWipeWatcher" => {
                    let i = objects(self, arg(0)).ok_or(Error::Native("AddWipeWatcher: no object".into()))?;
                    let l = self.objects[i].layer;
                    let test = (arg(3) as i32).signum();
                    let w = crate::wipe::Watcher { id: arg(1), value: f(2), test, once: arg(4) as u8 != 0, object: i };
                    self.layers[l].wipe.watchers.push(w);
                    0
                }
                // `0x4d5ea0` (object, id): the first watcher of the id on the object's layer goes (`0x4eaa30`)
                "LayerObj_DeleteWipeWatcher" => {
                    let l = self.obj(arg(0))?.layer;
                    let watchers = &mut self.layers[l].wipe.watchers;
                    if let Some(k) = watchers.iter().position(|w| w.id == arg(1)) {
                        watchers.remove(k);
                    }
                    0
                }
                // `0x4d5ec0`, `0x4d5fe0`, `0x4d6100`, `0x4d6850` (object, id, ms, keys, count, callback); the sprite's
                // (`0x4d6310`) adds a loop flag, the relative puppet's (`0x4d6750`) a word before the callback that
                // nothing reads
                "LayerObj_Model_AddAction_Spline"
                | "LayerObj_QuidditchCard_AddAction_Spline"
                | "LayerObj_Camera_AddAction_Spline"
                | "LayerObj_Sprite_AddAction_Spline"
                | "LayerObj_Puppet_AddAction_Spline"
                | "LayerObj_Puppet_AddAction_RelativeSpline" => {
                    let spline = match name {
                        "LayerObj_Model_AddAction_Spline" => Spline::Model,
                        "LayerObj_QuidditchCard_AddAction_Spline" => Spline::Card,
                        "LayerObj_Camera_AddAction_Spline" => Spline::Camera,
                        "LayerObj_Sprite_AddAction_Spline" => Spline::Sprite,
                        "LayerObj_Puppet_AddAction_Spline" => Spline::Puppet,
                        _ => Spline::PuppetRelative,
                    };
                    let looping = spline == Spline::Sprite && arg(6) as u8 != 0;
                    let callback = if spline == Spline::PuppetRelative { arg(6) } else { arg(5) };
                    let kind = ActionKind::Spline { spline, keys: arg(3), count: arg(4), looping };
                    self.add_action(arg(0), kind, arg(1), [0.0; 4], arg(2), callback)?
                }
                // the puppets (`0x4d6470`-`0x4d6700`, `qwc_feck::puppet`): (object, name, count, mode, ...); a count of
                // 1 plays once, anything else loops (the default flags); slot 0 unless an overlay names one
                "LayerObj_Puppet_PlayAnim"
                | "LayerObj_Puppet_PlayAnimCB"
                | "LayerObj_Puppet_PlayAnimFlags"
                | "LayerObj_Puppet_PlayAnimCBFlags"
                | "LayerObj_Puppet_PlayAnimCBID"
                | "LayerObj_Puppet_PlayAnimOverlay"
                | "LayerObj_Puppet_PlayAnimOverlayCB" => {
                    if let Some(i) = objects(self, arg(0)).filter(|i| self.puppets.contains_key(i)) {
                        let clip = String::from_utf8_lossy(&mem.cstr(arg(1))?).into_owned();
                        let (count, mode) = (arg(2), arg(3));
                        // (function, slot, sub, flag select, the slip word for an unknown select)
                        let (f, slot, sub, sel, slip) = match &name[16..] {
                            "PlayAnim" => (0, 0, 0, u32::MAX, 0),
                            "PlayAnimCB" => (arg(4), 0, 0, u32::MAX, 0),
                            "PlayAnimFlags" => (0, 0, 0, arg(4), arg(0)),
                            "PlayAnimCBFlags" => (arg(4), 0, 0, arg(5), 0),
                            "PlayAnimCBID" => (arg(4), 0, arg(5), arg(6), 0),
                            "PlayAnimOverlay" => (0, arg(4), 0, arg(5), arg(4)),
                            _ => (arg(4), arg(5), 0, arg(6), arg(5)),
                        };
                        let flags = puppet::play_flags(sel, count, slip);
                        self.puppet_play(mem, i, &clip, count, mode, flags, f, slot, sub)?;
                    }
                    0
                }
                // `0x4d66a0`, `0x4d66d0` (object, seconds)
                "LayerObj_Puppet_FadeDownSecondaryAnims" | "LayerObj_Puppet_FadeUpSecondaryAnims" => {
                    if let Some(p) = objects(self, arg(0)).and_then(|i| self.puppets.get_mut(&i)) {
                        p.character.fade_secondary(name.contains("FadeUp"), f(1));
                    }
                    0
                }
                // `0x4d6700` -> `0x468250` (object, x, y, z)
                "LayerObj_Puppet_LookAtPosition" => {
                    if let Some(p) = objects(self, arg(0)).and_then(|i| self.puppets.get_mut(&i)) {
                        p.look_at = Some([f(1), f(2), f(3)]);
                    }
                    0
                }
                "DataStore_CreateEntry" => {
                    // (key, type, count, stride or 0, &entry): fails when the key exists (`0x4d6bc0`)
                    if self.entry(arg(0)).is_none() {
                        let kind = arg(1);
                        let stride = match arg(3) as u16 {
                            0 => TYPE_SIZE.get(kind as usize).copied().unwrap_or(0),
                            s => s,
                        };
                        let count = arg(2) as u16;
                        let addr = mem.alloc(count as usize * stride as usize);
                        self.datastore.push(Entry { key: arg(0), kind, count, stride, addr });
                        if arg(4) != 0 {
                            mem.set_u32(arg(4), addr)?;
                        }
                    }
                    0
                }
                "DataStore_GetU8"
                | "DataStore_GetS8"
                | "DataStore_GetU16"
                | "DataStore_GetS16"
                | "DataStore_GetU32"
                | "DataStore_GetS32"
                | "DataStore_GetFP32"
                | "DataStore_GetUserType" => match self.entry(arg(0)) {
                    // (key, index): element `index` (no bounds check in the original)
                    Some(e) => {
                        let a = e.addr.wrapping_add(e.stride as u32 * arg(1));
                        match &name[13..] {
                            "U8" | "S8" => mem.u8(a)? as u32,
                            "U16" | "S16" => mem.u16(a)? as u32,
                            "UserType" => a,
                            _ => mem.u32(a)?,
                        }
                    }
                    None => 0,
                },
                "DataStore_SetU8" | "DataStore_SetS8" | "DataStore_SetU16" | "DataStore_SetS16"
                | "DataStore_SetU32" | "DataStore_SetS32" | "DataStore_SetFP32" => {
                    // (key, value, index); `QWC_DS_LOG` says each
                    if std::env::var_os("QWC_DS_LOG").is_some() {
                        eprintln!("{name}({:#010x}, {:#x}, {})", arg(0), arg(1), arg(2));
                    }
                    if let Some(e) = self.entry(arg(0)) {
                        let a = e.addr.wrapping_add(e.stride as u32 * arg(2));
                        match &name[13..] {
                            "U8" | "S8" => mem.set_u8(a, arg(1) as u8)?,
                            "U16" | "S16" => mem.set_u16(a, arg(1) as u16)?,
                            _ => mem.set_u32(a, arg(1))?,
                        }
                    }
                    0
                }
                "DataStore_SetUserType" => {
                    // (key, source, index): copies one element (`0x4d6de0`)
                    if let Some(e) = self.entry(arg(0)) {
                        let a = e.addr.wrapping_add(e.stride as u32 * arg(2));
                        let bytes = mem.read(arg(1), e.stride as u32)?.to_vec();
                        mem.write(a, &bytes)?;
                    }
                    0
                }
                "FECK_CalcCRC" => {
                    let b = mem.read(arg(0), arg(1))?.to_vec();
                    qwc_formats::flow::name_hash_seed(&b, arg(2))
                }
                "FECK_CalcCRCFromCHAR" => {
                    let s = mem.cstr(arg(0))?;
                    if s.is_empty() {
                        0
                    } else {
                        qwc_formats::flow::name_hash_seed(&s, 0)
                    }
                }
                // `0x4d2e50`: the bytes of a 16-bit string, without its terminator
                "FECK_CalcCRCFromWCHAR" => {
                    let mut n = 0;
                    while arg(0) != 0 && mem.u16(arg(0) + 2 * n)? != 0 {
                        n += 1;
                    }
                    if n == 0 {
                        0
                    } else {
                        let b = mem.read(arg(0), 2 * n)?.to_vec();
                        qwc_formats::flow::name_hash_seed(&b, 0)
                    }
                }
                // `0x4d2e90` returns its second argument
                "FECK_CheckCRC" => arg(1),
                "FECK_Printf" => {
                    let fmt = mem.cstr(arg(0))?;
                    let line = printf(mem, &fmt, &args[1.min(args.len())..]);
                    self.log.push(line);
                    0
                }
                "General_FindObjectInLayer" => match layer::layer_index(arg(0)).filter(|&l| l < self.layers.len()) {
                    Some(l) => self.layers[l]
                        .objects
                        .iter()
                        .find(|&&k| self.objects[k].id == arg(1))
                        .map_or(0, |&k| layer::object_handle(k)),
                    None => 0,
                },
                "General_FindLayer" => {
                    // `0x4d3310` (hash, &screen, &layer): 0 when found, else -1. The screen is reported as the layer
                    // itself (scripts only pass it back to natives that take a layer).
                    match self.layers.iter().position(|l| l.alive && l.hash == arg(0)) {
                        Some(l) => {
                            for out in [arg(1), arg(2)] {
                                if out != 0 {
                                    mem.set_u32(out, layer::layer_handle(l))?;
                                }
                            }
                            0
                        }
                        None => u32::MAX,
                    }
                }
                // `0x55f930`: 1 on the PC
                "General_GetPlatformType" => 1,
                "General_LoadSetIDFromCRC" => self.load_sets.iter().position(|&h| h == arg(0)).unwrap_or(0) as u32,
                // (object, exit) `0x4d31e0`: blocks the object's layer's input; the screen takes the exit when active
                // the flow natives (`0x4d31e0`-`0x4d32b0`) act through the calling object's layer's screen; the screen
                // manager (`qwc_feck::screens`) takes the requests. `RequestExit` blocks the calling layer's input itself.
                "General_RequestExit" => {
                    if let Some(i) = objects(self, arg(0)) {
                        let layer = self.objects[i].layer;
                        self.input_blocked[layer] = true;
                        self.requests.push(Request::Exit { layer, exit: arg(1), finish: arg(2) as u8 != 0 });
                    }
                    0
                }
                "General_RequestExitToNode" | "General_RequestJumpToNode" => {
                    if let Some(i) = objects(self, arg(0)) {
                        let (layer, hash, finish) = (self.objects[i].layer, arg(1), arg(2) as u8 != 0);
                        self.requests.push(if name.ends_with("ToNode") && name.contains("Exit") {
                            Request::ExitToNode { layer, hash, finish }
                        } else {
                            Request::JumpToNode { layer, hash, finish }
                        });
                    }
                    0
                }
                "General_PreTransitionDone" | "General_ReadyToExit" => {
                    if let Some(i) = objects(self, arg(0)) {
                        let layer = self.objects[i].layer;
                        self.requests.push(if name == "General_ReadyToExit" {
                            Request::ReadyToExit { layer }
                        } else {
                            Request::PreTransitionDone { layer }
                        });
                    }
                    0
                }
                "General_SendAudioEvent" => {
                    self.audio_events.push(arg(0));
                    0
                }
                // text (`0x4d6410`-`0x4d6b10`): the string id `+0xbc`, its index `+0xe8` and the effect flags `+0xc8`;
                // setting the id, or forcing it, formats the string again (`0x4dd6d0`). `SetStringID` keeps the index.
                // `0x4d6410`, shared by text and sprites: `+0xbc`
                "LayerObj_Text_GetStringID" | "LayerObj_Sprite_GetResourceID" => {
                    objects(self, arg(0)).map_or(0, |i| self.objects[i].slot(0xbc))
                }
                "LayerObj_Text_SetStringID" => {
                    self.obj(arg(0))?.set_slot(0xbc, arg(1));
                    self.reformat(mem, objects(self, arg(0)).unwrap());
                    0
                }
                "LayerObj_Text_SetStringIDWithIndex" => {
                    let o = self.obj(arg(0))?;
                    o.set_slot(0xbc, arg(1));
                    o.set_slot(0xe8, arg(2));
                    self.reformat(mem, objects(self, arg(0)).unwrap());
                    0
                }
                "LayerObj_Text_ForceReformatting" => {
                    self.obj(arg(0))?;
                    self.reformat(mem, objects(self, arg(0)).unwrap());
                    0
                }
                "LayerObj_Text_SetEffectFlag" | "LayerObj_Text_ResetEffectFlag" => {
                    let o = self.obj(arg(0))?;
                    let v = o.slot(0xc8);
                    o.set_slot(0xc8, if name.ends_with("_SetEffectFlag") { v | arg(1) } else { v & !arg(1) });
                    0
                }
                // menus (`0x4d6950`-`0x4d69b0`): the current choice `+0xc0` (an object) and the start index `+0xc4`.
                // `0x4d6950` also answers an animated sprite's resource id (its `+0xc0`)
                "LayerObj_Menu_GetCurrentSelection" | "LayerObj_AnimatedSprite_GetResourceID" => {
                    objects(self, arg(0)).map_or(0, |i| self.objects[i].slot(0xc0))
                }
                // `0x4d69d0` -> `0x4db900`
                "LayerObj_Menu_ForceChildUpdate" => {
                    if let Some(m) = objects(self, arg(0)) {
                        self.child_update(m);
                    }
                    0
                }
                "LayerObj_Menu_GetCurrentSelectionIndex" => objects(self, arg(0)).map_or(0, |i| {
                    let cur = self.objects[i].slot(0xc0);
                    self.objects[i].children.iter().position(|&k| layer::object_handle(k) == cur).unwrap_or(0) as u32
                }),
                // `0x4d69a0` / `0x4d6990`, a menu's `+0x44` and `+0x48` (`0x4dbd00`, `0x4dbcc0`): a highlightable
                // child of the menu becomes its choice (0); anything else answers 1. **[I]** objects other than
                // menus (another vtable) answer 0
                "LayerObj_Menu_SetCurrentSelection" | "LayerObj_Menu_SetCurrentSelectionIndex" => {
                    match objects(self, arg(0)).filter(|&m| self.objects[m].kind == layer::Kind::Menu) {
                        None => 0,
                        Some(m) => {
                            let kids = &self.objects[m].children;
                            let c = if name == "LayerObj_Menu_SetCurrentSelection" {
                                objects(self, arg(1)).filter(|c| kids.contains(c))
                            } else {
                                let n = arg(1) as i16;
                                (n >= 0).then(|| kids.get(n as usize).copied()).flatten()
                            };
                            match c.filter(|&c| self.objects[c].flags & flag::HIGHLIGHTABLE != 0) {
                                Some(c) => {
                                    self.select(mem, m, Some(c))?;
                                    0
                                }
                                None => 1,
                            }
                        }
                    }
                }
                // `0x4d6a50` (slider, value): the value's step, `(v - min) / (max - min) * (count - 1)` truncated
                // and clamped (**[T]** 2026-10-23; the original's x87 keeps the product exact, so 1/9 of the way is
                // step 1: worked in doubles here); then the value handler (`+0xe4`) with (slider, step, `+0xcc`, 0)
                "LayerObj_Slider_SetValue" => {
                    if let Some(i) = objects(self, arg(0)) {
                        let o = &mut self.objects[i];
                        let count = o.slot(0xc4) as i32;
                        let (min, max) = (f32::from_bits(o.slot(0xbc)) as f64, f32::from_bits(o.slot(0xc0)) as f64);
                        let step = (f(1) as f64 - min) / (max - min) * count.wrapping_sub(1) as f64;
                        let at = layer::slider_clamp(layer::ftol(step as f32), count);
                        o.set_slot(0xc8, at);
                        let cb = o.slot(0xe4);
                        if cb != 0 {
                            then = Some((o.layer, cb, vec![arg(0), at, o.slot(0xcc), 0]));
                        }
                    }
                    0
                }
                "LayerObj_Menu_SetStartSelectionIndex" => {
                    self.obj(arg(0))?.set_slot(0xc4, arg(1) & 0xffff);
                    0
                }
                // `0x4d5dd0`: the object's layer stops taking input
                "LayerObj_SetInputBlock" => {
                    if let Some(i) = objects(self, arg(0)) {
                        let l = self.objects[i].layer;
                        self.input_blocked[l] = arg(1) as u8 != 0;
                    }
                    0
                }
                // `0x4d6420`: the resource id; the texture it names is looked up by the renderer
                "LayerObj_Sprite_SetResourceID" => {
                    self.obj(arg(0))?.set_slot(0xbc, arg(1));
                    0
                }
                // `0x4d69e0`: back to the first frame
                "LayerObj_AnimatedSprite_ResetAnimation" => {
                    let o = self.obj(arg(0))?;
                    let start = o.slot(0xcc);
                    o.set_slot(0xd0, start);
                    o.set_slot(0xd4, 0);
                    0
                }
                // `0x4d6a00` (object, label): when the label is set, the script goes there and steps at once
                "LayerObj_AnimatedSprite_GotoLabel" => {
                    let o = self.obj(arg(0))?;
                    let to = o.slot(0xd8 + 4 * (arg(1) & 0xff) as u16);
                    if to != 0 {
                        o.set_slot(0xd0, to);
                        layer::animate(o);
                    }
                    0
                }
                "LayerObj_AnimatedSprite_SetFrame" => {
                    self.obj(arg(0))?.set_slot(0xc8, arg(1) & 0xff);
                    0
                }
                // `0x4d6a30` (object, id, frames) -> `0x4db5c0`: the frames' textures are let go; with a frame count
                // the sprite starts again from frame 0 and its loop count is cleared. The renderer looks the frame up
                // (`0x4dafa0`)
                "LayerObj_AnimatedSprite_SetResourceID" => {
                    let o = self.obj(arg(0))?;
                    o.set_slot(0xc0, arg(1));
                    if (arg(2) as i32) > 0 {
                        o.set_slot(0xc4, arg(2));
                        o.set_slot(0xc8, 0);
                        o.set_slot(0xec, 0);
                    }
                    0
                }
                // `0x4d5fd0`, shared: `+0xbc` (a model's resource, a camera's interest object)
                "LayerObj_Model_SetResource" | "LayerObj_Camera_SetInterest" => {
                    self.obj(arg(0))?.set_slot(0xbc, arg(1));
                    0
                }
                // `0x4d5fc0`, `0x4d60e0`: bytes
                "LayerObj_Model_SetRotationMode" => {
                    self.obj(arg(0))?.set_slot(0xd4, arg(1) & 0xff);
                    0
                }
                "LayerObj_QuidditchCard_SetRotationMode" => {
                    self.obj(arg(0))?.set_slot(0xd8, arg(1) & 0xff);
                    0
                }
                // `0x4d60f0`
                "LayerObj_Camera_SetFocusInterest" => {
                    self.obj(arg(0))?.set_slot(0xc0, arg(1));
                    0
                }
                // `0x4d6b20` (object, description): 19 words copied to `+0xc0`, and `+0x110` set (a new description)
                "LayerObj_Particle_SetDesc" => {
                    let words = (0..19).map(|k| mem.u32(arg(1) + 4 * k)).collect::<std::result::Result<Vec<_>, _>>()?;
                    let o = self.obj(arg(0))?;
                    for (k, w) in words.into_iter().enumerate() {
                        o.set_slot(0xc0 + 4 * k as u16, w);
                    }
                    o.set_slot(0x110, 1);
                    0
                }
                "General_IsPaused" => self.paused as u32,
                "General_SetPaused" => {
                    self.paused = arg(0) != 0;
                    self.engine.push(EngineEvent::Paused(self.paused));
                    0
                }
                "General_FrontendEntry" => {
                    let on = arg(0) != 0;
                    self.engine.push(EngineEvent::FrontendEntry(on));
                    self.engine.push(EngineEvent::FrontendScene(on));
                    self.scene = on;
                    if on {
                        self.engine.push(EngineEvent::ScreenFade(false));
                        self.hoops = true;
                    }
                    0
                }
                "General_FrontendCutSceneEntry" => {
                    self.engine.push(EngineEvent::FrontendScene(arg(0) == 0));
                    self.scene = arg(0) == 0;
                    0
                }
                "General_LoadScreen" => {
                    if arg(0) == 0 {
                        self.engine.push(EngineEvent::LoadScreen);
                    }
                    0
                }
                "General_TimeoutEnable" => {
                    self.engine.push(EngineEvent::TimeoutEnable(arg(0)));
                    0
                }
                // `0x4d3b30`: 0 when switched (or already current), `0x80020005` for a language the data has no table
                // for; the held strings change language at once (`Strings::switch`)
                "General_ChangeLanguage" => {
                    let hash = arg(0);
                    if self.language == hash {
                        0
                    } else if self.languages.is_empty() || self.switch_language(mem, hash) {
                        self.language = hash;
                        self.engine.push(EngineEvent::Language(hash));
                        0
                    } else {
                        0x8002_0005
                    }
                }
                "General_AudioVolumeEvent" => {
                    self.engine.push(EngineEvent::AudioVolume { channel: arg(0) as u8, value: arg(1) });
                    0
                }
                "General_PlayResultsSpeech" => {
                    self.engine.push(EngineEvent::ResultsSpeech([arg(0), arg(1), arg(2)]));
                    0
                }
                "General_PlayMovie" => {
                    let name = if arg(0) != 0 {
                        String::from_utf8_lossy(&mem.cstr(arg(0))?).into_owned()
                    } else {
                        String::new()
                    };
                    self.engine.push(EngineEvent::PlayMovie { name, flag: arg(1) });
                    0
                }
                "General_RefreshWavyFlags" => {
                    self.engine.push(EngineEvent::RefreshWavyFlags);
                    0
                }
                // `0x4d3930` / `0x4d39b0`: DataStore `0x31f694a1` says which; the non-Asian one also clears three
                // control settings (`0x1043770`, `0x1043784`; **[I]** not modelled)
                "General_SetAsianInput" | "General_SetNonAsianInput" => {
                    let asian = name == "General_SetAsianInput";
                    if let Some(e) = self.entry(0x31f694a1) {
                        mem.set_u8(e.addr, asian as u8)?;
                    }
                    let folder = if asian { "FrontEnd/Common/AControl" } else { "FrontEnd/Common/Controls" };
                    self.engine.push(EngineEvent::ControlIcons(folder.to_string()));
                    0
                }
                "General_Light_SetAmbientColour" => {
                    self.lights.ambient = [f(0), f(1), f(2)];
                    0
                }
                "General_Light_SetDiffuseColour_0"
                | "General_Light_SetDiffuseColour_1"
                | "General_Light_SetDiffuseColour_2" => {
                    let k = (name.as_bytes()[name.len() - 1] - b'0') as usize;
                    self.lights.diffuse[k] = [f(0), f(1), f(2)];
                    0
                }
                "General_Light_SetDirection_0" | "General_Light_SetDirection_1" | "General_Light_SetDirection_2" => {
                    // `0x4d3560`: normalised, a zero vector left at zero
                    let k = (name.as_bytes()[name.len() - 1] - b'0') as usize;
                    let (x, y, z) = (f(0), f(1), f(2));
                    let len = (z * z + y * y + x * x).sqrt();
                    let s = if len != 0.0 { 1.0 / len } else { 0.0 };
                    self.lights.direction[k] = [s * x, s * y, s * z];
                    0
                }
                // `0x4d33e0`-`0x4d3500` (&x, &y, &z): what the setters stored (directions normalised)
                "General_Light_GetAmbientColour"
                | "General_Light_GetDiffuseColour_0"
                | "General_Light_GetDiffuseColour_1"
                | "General_Light_GetDiffuseColour_2"
                | "General_Light_GetDirection_0"
                | "General_Light_GetDirection_1"
                | "General_Light_GetDirection_2" => {
                    let k = (name.as_bytes()[name.len() - 1].wrapping_sub(b'0')) as usize;
                    let v = match &name[17..20] {
                        "Amb" => self.lights.ambient,
                        "Dif" => self.lights.diffuse[k],
                        _ => self.lights.direction[k],
                    };
                    for (n, x) in v.iter().enumerate() {
                        mem.set_u32(arg(n), x.to_bits())?;
                    }
                    0
                }
                "General_EnableForceFeedback" => {
                    self.force_feedback = arg(0) as u8 != 0;
                    0
                }
                // `0x4d32f0` (object, flag): every layer of the object's screen
                "General_BlockInput" => {
                    if let Some(i) = objects(self, arg(0)) {
                        let l = self.objects[i].layer;
                        let screen = self.layers[l].screen;
                        for k in 0..self.layers.len() {
                            if k == l || (screen.is_some() && self.layers[k].screen == screen) {
                                self.input_blocked[k] = arg(1) as u8 != 0;
                            }
                        }
                    }
                    0
                }
                "General_TotalQuidditchCards" => self.total_quidditch_cards(mem)?,
                // `0x4d37b0`: which loading-screen controller to show, from three unlock bits; also sets DataStore
                // `0xe659b2fc` to 325
                "General_SetLoadingController" => {
                    let bit = |fe: &FrontEnd, mem: &Memory, row: u32, b: u32| -> u32 {
                        fe.entry(KEY_UNLOCK_BITS)
                            .and_then(|e| mem.u16(e.addr + e.stride as u32 * row).ok())
                            .map_or(0, |v| (v as u32 >> b) & 1)
                    };
                    let (b2, b3, b4) = (bit(self, mem, 1, 2), bit(self, mem, 0, 3), bit(self, mem, 3, 3));
                    self.set_datastore_u32(mem, 0xe659b2fc, 0x145);
                    match b4 << 2 | b3 << 1 | b2 {
                        0 => 1,
                        1 => 4,
                        2 => 5,
                        3 => 2,
                        _ => 0,
                    }
                }
                // `0x4d43f0` -> `0x5110e0` (width, height, bits): the index of a matching mode of the current adapter
                // (formats 0x15/0x16 for 32 bits, 0x17/0x18 for 16), or -1
                "General_VerifyDisplay" => self
                    .display_modes
                    .iter()
                    .position(|&(w, h, d)| w == arg(0) && h == arg(1) && d == arg(2) && (d == 16 || d == 32))
                    .map_or(u32::MAX, |i| i as u32),
                // `0x4d4a40`: a word of the pad's input device when one is connected. **[I]** no devices: 0
                "General_NoAxesPlayerControls" => 0,
                // the PC build's shared empty function (`0x53f020`)
                "General_SetDepthOfField"
                | "General_SetScreenOffsets"
                | "GBALink_Reset"
                | "General_Mudlark"
                | "General_MoveScreenOffset"
                | "General_MoveScreenOffsetUp"
                | "General_MoveScreenOffsetDown"
                | "General_MoveScreenOffsetLeft"
                | "General_MoveScreenOffsetRight" => 0,
                // `0x55f930` (shared with `GetPlatformType`) and `0x540b90`, the shared `return 0`: the PC build
                // reports no pads
                "General_GetBuildType" => 1,
                "General_GetNumPadsConnected" => 0,
                // `0x4d4aa0`
                "GBALink_GetTimeSinceLastKeepAlive" => u32::MAX,
                // `0x4d5040`: a flag that starts set (no native clears it)
                "General_UseRenderToTexture" => self.render_to_texture as u32,
                "General_GetCurrentDisplaymode" => self.display_mode,
                // the Video options pass the mode `General_VerifyDisplay` found (`TeamTentOptionsVideo`, `0x2199`);
                // **[I]** it becomes the current mode at once (the host changes the window)
                "General_ChangeDisplay" => {
                    self.display_mode = arg(0);
                    self.engine.push(EngineEvent::ChangeDisplay(arg(0)));
                    0
                }
                "General_EnableWideScreen" => {
                    self.widescreen = arg(0) as u8 != 0;
                    self.engine.push(EngineEvent::WideScreen(self.widescreen));
                    0
                }
                // `0x4d3880`: -1 clears the value only when it is 2
                "General_PuppetHack" => {
                    if arg(0) != u32::MAX {
                        self.puppet_hack = arg(0);
                    } else if self.puppet_hack == 2 {
                        self.puppet_hack = 0;
                    }
                    0
                }
                "General_SetExitRequestFlag" => {
                    self.exit_request = true;
                    0
                }
                "General_ToggleInputOnOff" => {
                    self.input_toggle = arg(0) as u8;
                    0
                }
                "General_ScreenFade" => {
                    self.engine.push(EngineEvent::ScreenFade(arg(0) != 0));
                    0
                }
                "General_PlayFrontendSpeech" => {
                    self.engine.push(EngineEvent::FrontendSpeech([arg(0), arg(1), arg(2)]));
                    0
                }
                "General_PlayCutScene" => {
                    self.engine.push(EngineEvent::PlayCutScene(arg(0)));
                    0
                }
                // `0x4d5050`: the texture `name` (loaded with the folder as the load's path, `0x4c7760`) on the card
                // window's material; an empty name gives the engine's default (`[0xeaa6d4]`)
                "General_SetQuidditchCardTexture" => {
                    let folder = String::from_utf8_lossy(&mem.cstr(arg(0))?).into_owned();
                    let name = String::from_utf8_lossy(&mem.cstr(arg(1))?).into_owned();
                    if std::env::var_os("QWC_CARD_LOG").is_some() {
                        eprintln!("General_SetQuidditchCardTexture({folder:?}, {name:?})");
                    }
                    self.card_texture = (!name.is_empty()).then_some((folder, name));
                    0
                }
                "General_PlayCutSceneSpeech" => {
                    self.engine.push(EngineEvent::CutSceneSpeech([arg(0), arg(1)]));
                    0
                }
                "General_PlayChallengeSpeech" => {
                    self.engine.push(EngineEvent::ChallengeSpeech([arg(0), arg(1)]));
                    0
                }
                "General_LoadSet" => {
                    self.engine.push(EngineEvent::LoadSet(arg(0)));
                    0
                }
                "General_UnloadSet" => {
                    self.engine.push(EngineEvent::UnloadSet(arg(0)));
                    0
                }
                "General_GameInitGameAudio" => {
                    self.engine.push(EngineEvent::InitGameAudio);
                    0
                }
                // `0x4d3cb0`: also clears the first word of DataStore `0x5b41ee60`
                "General_InitMatch" => {
                    let v = match self.entry(0x1e394bdc) {
                        Some(e) => mem.u32(e.addr)? as i32,
                        None => 0,
                    };
                    if let Some(e) = self.entry(0x5b41ee60) {
                        mem.set_u32(e.addr, 0)?;
                    }
                    self.engine.push(EngineEvent::InitMatch(v >= 0));
                    0
                }
                "General_DeInitMatch" => {
                    self.engine.push(EngineEvent::DeInitMatch);
                    0
                }
                // `0x4d4db0` (group, index, on): bit `index` of row `group`; setting a card also unlocks it, clearing
                // leaves the unlock
                "General_SetQuidditchCard" => {
                    let bit = (1u32 << (arg(1) & 31)) as u16;
                    let at = |fe: &FrontEnd, key: u32| fe.entry(key).map(|e| e.addr + e.stride as u32 * arg(0));
                    let (unlock, cards) = (at(self, KEY_UNLOCK_BITS), at(self, KEY_CARD_BITS));
                    let mut u = unlock.map_or(Ok(0), |a| mem.u16(a))?;
                    let mut c = cards.map_or(Ok(0), |a| mem.u16(a))?;
                    if arg(2) != 0 {
                        u |= bit;
                        c |= bit;
                    } else {
                        c &= !bit;
                    }
                    if let Some(a) = unlock {
                        mem.set_u16(a, u)?;
                    }
                    if let Some(a) = cards {
                        mem.set_u16(a, c)?;
                    }
                    0
                }
                // `0x4d36b0`: the seed becomes the 15-bit result; a float in [0, 1]
                "rand" => {
                    self.rand_seed = (self.rand_seed.wrapping_mul(0x343fd).wrapping_add(0x269ec3) >> 16) & 0x7fff;
                    (self.rand_seed as f32 * (1.0 / 32767.0)).to_bits()
                }
                "srand" => {
                    self.rand_seed = arg(0);
                    0
                }
                // the key bindings (`0x4d46b0`-`0x4d4a90`, `qwc_feck::keys`); the player is a byte
                "General_GetControlKey" => self.keys.get(arg(0) & 0xff, arg(1)),
                "General_SetControlKey" => {
                    self.keys.set(arg(0) & 0xff, arg(1), arg(2));
                    0
                }
                // `0x508960`: the key just pressed. **[I]** the host's input, cleared once read
                "General_GetRawKey" => std::mem::take(&mut self.keys.raw_key),
                "General_IsKeyFree" => self.keys.is_free(arg(0) & 0xff, arg(1)) as u32,
                "General_IsKeyTypeCompatible" => Keys::compatible(arg(0), arg(1)) as u32,
                "General_SaveKeyBindings" => {
                    let ok = self.keys.save();
                    self.log.push(if ok { "Key config save OK" } else { "Key config save failed" }.to_string());
                    0
                }
                "General_LoadKeyBindings" => {
                    if self.keys.load() {
                        self.log.push("Key config read OK".to_string());
                    }
                    0
                }
                // `0x4d3700`: runs a console command line
                "General_ExecuteCLICommand" => {
                    if arg(0) != 0 {
                        let c = mem.cstr(arg(0))?;
                        if !c.is_empty() {
                            self.commands.push(String::from_utf8_lossy(&c).into_owned());
                        }
                    }
                    0
                }
                "General_EnableStadiumRender" => {
                    self.stadium_render = arg(0) as u8 != 0;
                    0
                }
                // the memory card (`0x4d3b40`-`0x4d3ca0`, `qwc_feck::memcard`)
                "Memcard_EnableMemoryCards" => {
                    self.mc_enable(arg(0) as u8 != 0);
                    0
                }
                // (object, function): the callback runs in the object's layer
                "Memcard_RegisterMessageCB" | "Memcard_RegisterStateCB" => {
                    let cb = objects(self, arg(0)).map(|i| (self.objects[i].layer, arg(1)));
                    if name == "Memcard_RegisterMessageCB" {
                        self.memcard.message_cb = cb;
                    } else {
                        self.memcard.state_cb = cb;
                    }
                    0
                }
                // `0x4d3be0`: taken only while a message waits. **[I]** the result is the waiting flag it read
                "Memcard_Reply" => {
                    let was = self.memcard.waiting;
                    if was {
                        self.memcard.waiting = false;
                        self.memcard.reply = arg(0);
                    }
                    was as u32
                }
                "Memcard_ChangeState" => {
                    if let Some(s) = memcard::machine_state(arg(0)) {
                        self.memcard.state = s;
                    }
                    0
                }
                "Memcard_ActivateSlot" => {
                    self.memcard.active_slot = arg(0);
                    memcard::activate(&self.memcard.buffer, &mut self.datastore, mem, arg(0))
                }
                // `0x4d3c20`
                "Memcard_PrepareSlot" => {
                    memcard::pack(&mut self.memcard.buffer, &self.datastore, mem, &memcard::SAVE_KEYS, false);
                    memcard::summaries(&self.memcard.buffer, &self.datastore, mem);
                    0
                }
                "Memcard_DeleteSlot" => {
                    memcard::delete_slot(&mut self.memcard.buffer, &self.datastore, mem, arg(0));
                    0
                }
                // the PC build's shared `return 0` (`0x540b90`)
                "Memcard_SetCheckingMessageDisplay" => 0,
                // `0x4d3c80`: a countdown from 4, always answering 0
                "Memcard_CheckCard" => {
                    let c = &mut self.memcard.check;
                    *c = if *c == 0 { 4 } else { *c - 1 };
                    0
                }
                // `0x4ebdb0`: the string with this id, narrowed to bytes, is the save's display name
                "Memcard_SetSaveGameName" => {
                    let view = DataStoreView { entries: &self.datastore, mem };
                    if let Some(strings) = self.strings.as_mut() {
                        if let Ok(Some(h)) = strings.acquire(arg(0), 0, &view) {
                            let t = strings.text(h).unwrap_or(&[]);
                            self.memcard.save_name = t.iter().map(|&c| c as u8).collect();
                            strings.release(h);
                        }
                    }
                    0
                }
                // the PC build's Game Boy Advance link natives are the shared `return 0` (`0x540b90`)
                "GBALink_IsConnected" | "GBALink_SniffQCardCancelPacket" => 0,
                "GBALink_SendQCardPacket" => {
                    self.gba_packet(mem, arg(1), arg(2), arg(3));
                    0
                }
                // `0x4d38b0` (player): the pad interface (`0x6688bc`, vtable `0x5acc24`) gives player 0 or 1 the
                // controller of its number while the keyboard and the mouse are open (`+0x1c`, `0x4c9640`; else
                // 0xff, answered 0), and `0x4c8d10` asks the same of that controller. Its first call also sets up a
                // static (`[0x10600f0]` bit 0, `[0x1060058]`) that nothing here reads
                "General_IsPlayersControllerPresent" => (self.keyboard_and_mouse && (arg(0) as u8) < 2) as u32,
                // `0x4d4d60` logs an unknown packet and fails
                "GBALink_ReceiveQCardPacket" => u32::MAX,
                _ => unreachable!("{name} is in NATIVES"),
            })
        };
        let r = run().map_err(|e| match e {
            Error::Native(s) => Error::Native(format!("{name}: {s}")),
            e => e,
        })?;
        // `0x4dbc20`: in the object's layer, while that layer is live
        if let Some((l, hash, a)) = then {
            if self.layers.get(l).is_some_and(|x| x.alive) {
                let script = self.layers[l].script;
                vm.execute(self, script, hash, &a)?;
            }
        }
        Ok(r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_animate_action_steps_a_sprite_through_its_frames() {
        use crate::draw::{bind_sprite, sprite_tex, Tex};
        let kind = ActionKind::Animate { count: 4, dir: 1, ping_pong: false, base: None, last: -999 };
        let mut a = Action { kind, id: 1, elapsed: 0, duration: 8, from: [0.0; 4], to: [0.0; 4], callback: 0 };
        let mut o = Object::new(layer::Kind::Sprite, 0);
        o.set_slot(0xbc, 7);
        let mem = Memory::new();
        // not drawn yet: nothing bound, nothing to count from
        update(&mut o, &mut a, 0.5, &mem).unwrap();
        assert_eq!(sprite_tex(&o).frame, 0);
        // drawn: frames 4 × t on from the bound one, ending on the fourth after it
        bind_sprite(&mut o, Tex { resource: 7, frame: 0, script: 0 }, [8, 8]);
        update(&mut o, &mut a, 0.5, &mem).unwrap();
        assert_eq!(sprite_tex(&o), Tex { resource: 7, frame: 2, script: 0 });
        update(&mut o, &mut a, 0.9, &mem).unwrap();
        assert_eq!(sprite_tex(&o).frame, 3);
        assert!(!finish(&mut o, &mut a));
        assert_eq!(sprite_tex(&o).frame, 4);
        // back and forth: it turns round at the end and runs again, now from the last frame down
        a.kind = ActionKind::Animate { count: 4, dir: 1, ping_pong: true, base: Some(0), last: 4 };
        assert!(finish(&mut o, &mut a));
        update(&mut o, &mut a, 0.25, &mem).unwrap();
        assert_eq!(sprite_tex(&o).frame, 3);
    }

    #[test]
    fn frames_and_printf() {
        assert_eq!(frames(300), 7);
        assert_eq!(frames(1000), 24);
        let mut m = Memory::new();
        let s = m.alloc_cstr(b"abc");
        assert_eq!(
            printf(&m, b"%d %s %x %5.2f %%", &[(-3i32) as u32, s, 255, 1.5f32.to_bits()]),
            "-3 abc ff 1.500000 %"
        );
    }

    #[test]
    fn actions_run_and_finish() {
        let mut o = layer::Object::new(layer::Kind::Sprite, 0);
        let a = Action {
            kind: ActionKind::Alpha,
            id: 1,
            elapsed: 0,
            duration: 4,
            from: [1.0, 0.0, 0.0, 0.0],
            to: [0.0; 4],
            callback: 0,
        };
        let mut mem = Memory::new();
        update(&mut o, &mut a.clone(), 0.25, &mem).unwrap();
        assert_eq!(o.colour[3], 0.75);
        let mut t = Action { kind: ActionKind::Timer, from: [2.0, 0.0, 0.0, 0.0], ..a.clone() };
        assert!(finish(&mut o, &mut t));
        assert!(!finish(&mut o, &mut t));
        let mut forever = Action { kind: ActionKind::Timer, from: [0.0; 4], ..a.clone() };
        assert!(finish(&mut o, &mut forever) && finish(&mut o, &mut forever));
        // a model spline over keys at x = 0, 10, 20, 30 (count 3: key 0 leads in, the curve runs from key 1 to
        // key 2, and key 3 is read with no weight at the end)
        let keys = mem.alloc(4 * 28);
        for k in 0..4u32 {
            let words = [k as f32 * 10.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0];
            for (n, w) in words.iter().enumerate() {
                mem.set_u32(keys + 28 * k + 4 * n as u32, w.to_bits()).unwrap();
            }
        }
        let kind = ActionKind::Spline { spline: Spline::Model, keys, count: 3, looping: false };
        let mut m = layer::Object::new(layer::Kind::Model, 0);
        let mut s = Action { kind, ..a };
        update(&mut m, &mut s, 0.0, &mem).unwrap();
        assert_eq!(m.pos[0], 10.0);
        update(&mut m, &mut s, 0.5, &mem).unwrap();
        // catmull(0, 10, 20, 20) at 0.5: the last point repeats once the next would be the count
        assert_eq!(m.pos[0], 15.625);
        update(&mut m, &mut s, 1.0, &mem).unwrap();
        assert_eq!((m.pos[0], f32::from_bits(m.slot(0xd0))), (20.0, 1.0));
        assert!(!finish(&mut m, &mut s), "a spline does not repeat");
        assert_eq!(m.pos[0], 20.0, "and sets no end value");
    }
}

#[cfg(test)]
mod menu_tests {
    use super::*;
    use qwc_formats::vso::Layer as Node;

    /// A menu (flag 4: wrapping) of three choices, the middle one not highlightable.
    fn menu() -> (FrontEnd, Vm) {
        let mut fe = FrontEnd::new();
        let mut vm = Vm::new();
        let choice = |id, flags| Node::Object { kind: "NULL", fields: vec![], id, flags, children: vec![] };
        let tree = vec![Node::Object {
            kind: "BMNU",
            fields: vec![4],
            id: 1,
            flags: 0x80,
            children: vec![choice(10, 0x10), choice(11, 0), choice(12, 0x10)],
        }];
        let (mut objs, mut order) = (Vec::new(), Vec::new());
        let roots = layer::build(&tree, 0, None, &mut objs, &mut order, &mut |_: &[u32]| 0);
        fe.objects = objs;
        let (name, hash) = ("m.vso".into(), 0);
        let wipe = Default::default();
        fe.layers.push(Layer {
            script: 0,
            name,
            hash,
            refs: 1,
            alive: true,
            screen: None,
            roots,
            objects: order,
            wipe,
        });
        fe.input_blocked.push(false);
        vm.mem.alloc(4);
        (fe, vm)
    }

    #[test]
    fn menus_pick_and_move_their_choice() {
        let (mut fe, mut vm) = menu();
        fe.menu_construct(&mut vm.mem, 0).unwrap();
        assert_eq!(fe.objects[0].slot(0xc0), layer::object_handle(1));
        // every child hears whether it is the choice (`0x4db900`)
        assert_eq!(
            fe.queue.drain(..).collect::<Vec<_>>(),
            [FeEvent::Highlight { id: 10 }, FeEvent::Unhighlight { id: 11 }, FeEvent::Unhighlight { id: 12 }]
        );
        // forward skips the choice that cannot be highlighted
        fe.menu_step(&mut vm, 0, 0, true).unwrap();
        assert_eq!(fe.objects[0].slot(0xc0), layer::object_handle(3));
        assert_eq!(
            fe.queue.drain(..).collect::<Vec<_>>(),
            [FeEvent::Unhighlight { id: 10 }, FeEvent::Highlight { id: 12 }]
        );
        // and wraps back to the first
        fe.menu_step(&mut vm, 0, 0, true).unwrap();
        assert_eq!(fe.objects[0].slot(0xc0), layer::object_handle(1));
    }

    #[test]
    fn the_video_options_modes() {
        let mut fe = FrontEnd::new();
        let mut vm = Vm::new();
        let keys = [0x49d6_243d, 0x4925_4310, 0xd2a6_1cdc, 0xe47e_d966];
        for k in keys {
            let addr = vm.mem.alloc(4);
            fe.datastore.push(Entry { key: k, kind: 2, count: 1, stride: 4, addr });
        }
        fe.display_modes = vec![(800, 600, 32), (800, 600, 16), (1024, 768, 32), (1920, 1080, 32)];
        fe.note_display_modes(&mut vm.mem);
        let flags: Vec<u32> = keys.iter().map(|&k| fe.datastore_u32(&vm.mem, k).unwrap()).collect();
        assert_eq!(flags, [0, 1, 1, 0]);
        let verify = |fe: &mut FrontEnd, vm: &mut Vm, w, h, d| {
            fe.call(vm, name_hash("General_VerifyDisplay"), &[w, h, d], 0).unwrap()
        };
        assert_eq!(verify(&mut fe, &mut vm, 1024, 768, 32), 2);
        assert_eq!(verify(&mut fe, &mut vm, 640, 480, 32), u32::MAX);
        // the choice is passed on, and is the current mode
        fe.call(&mut vm, name_hash("General_ChangeDisplay"), &[2], 0).unwrap();
        assert_eq!(fe.engine.last(), Some(&EngineEvent::ChangeDisplay(2)));
        assert_eq!(fe.call(&mut vm, name_hash("General_GetCurrentDisplaymode"), &[], 0).unwrap(), 2);
    }

    #[test]
    fn natives_set_a_menu_choice() {
        let (mut fe, mut vm) = menu();
        let h = layer::object_handle;
        let mut call = |fe: &mut FrontEnd, n: &str, a: &[u32]| fe.call(&mut vm, name_hash(n), a, 0).unwrap();
        // index 1 cannot be highlighted; index 2 is the third child; past the end there is none
        assert_eq!(call(&mut fe, "LayerObj_Menu_SetCurrentSelectionIndex", &[h(0), 1]), 1);
        assert_eq!(call(&mut fe, "LayerObj_Menu_SetCurrentSelectionIndex", &[h(0), 2]), 0);
        assert_eq!(fe.objects[0].slot(0xc0), h(3));
        assert_eq!(call(&mut fe, "LayerObj_Menu_SetCurrentSelectionIndex", &[h(0), 5]), 1);
        // by object: only a child of this menu
        assert_eq!(call(&mut fe, "LayerObj_Menu_SetCurrentSelection", &[h(0), h(0)]), 1);
        assert_eq!(call(&mut fe, "LayerObj_Menu_SetCurrentSelection", &[h(0), h(1)]), 0);
        assert_eq!(fe.objects[0].slot(0xc0), h(1));
    }

    /// `docs/feck_natives.md`'s status column is the port's: every native ported is in `NATIVES`, every hash in
    /// the table is in `EXE_NATIVES`. The public repo has no `docs/`: there the test has nothing to check.
    #[test]
    fn natives_doc_matches_the_port() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/feck_natives.md");
        let Ok(doc) = std::fs::read_to_string(path) else {
            eprintln!("no docs/feck_natives.md: skipped");
            return;
        };
        let mut rows = 0;
        for row in doc.lines().filter(|l| l.starts_with("| `0x")) {
            let c: Vec<&str> = row.split('|').map(str::trim).collect();
            let hash = u32::from_str_radix(c[1].trim_matches('`').trim_start_matches("0x"), 16).unwrap();
            let (name, status) = (c[3], c[4]);
            assert!(EXE_NATIVES.contains(&hash), "{name}");
            let ported = NATIVES.contains(&name);
            let want = if name == "?" {
                "unused"
            } else if ported {
                "ported"
            } else {
                "left"
            };
            assert_eq!(status, want, "{name}");
            if name != "?" {
                assert_eq!(name_hash(name), hash, "{name}");
            }
            rows += 1;
        }
        assert_eq!(rows, EXE_NATIVES.len());
        assert!(NATIVES.iter().all(|n| EXE_NATIVES.contains(&name_hash(n))));
    }

    #[test]
    fn rand_and_cards() {
        let mut fe = FrontEnd::new();
        let mut vm = Vm::new();
        let r = |fe: &mut FrontEnd, vm: &mut Vm| f32::from_bits(fe.call(vm, name_hash("rand"), &[], 0).unwrap());
        // seed 1: (0x343fd + 0x269ec3) >> 16 & 0x7fff = 41, then (41 * 0x343fd + 0x269ec3) >> 16 & 0x7fff
        assert_eq!(r(&mut fe, &mut vm), 41.0 / 32767.0);
        assert_eq!(fe.rand_seed, 41);
        let next = (41u32.wrapping_mul(0x343fd).wrapping_add(0x269ec3) >> 16) & 0x7fff;
        assert_eq!(r(&mut fe, &mut vm), next as f32 / 32767.0);
        // 103 cards: rows of all ones but the last card of row 4
        let bits = vm.mem.alloc(30);
        for (i, &n) in CARDS_PER_ROW.iter().enumerate() {
            let v = if i == 4 { 0b0111 } else { (1u16 << n) - 1 };
            vm.mem.set_u16(bits + 2 * i as u32, v).unwrap();
        }
        let unlock = vm.mem.alloc(30);
        let total = vm.mem.alloc(4);
        fe.datastore.push(Entry { key: KEY_CARD_BITS, kind: 1, count: 15, stride: 2, addr: bits });
        fe.datastore.push(Entry { key: KEY_UNLOCK_BITS, kind: 1, count: 15, stride: 2, addr: unlock });
        fe.datastore.push(Entry { key: KEY_CARD_TOTAL, kind: 2, count: 1, stride: 4, addr: total });
        assert_eq!(fe.call(&mut vm, name_hash("General_TotalQuidditchCards"), &[], 0).unwrap(), 104);
        assert_eq!(vm.mem.u16(bits + 8).unwrap(), 0b1111);
        assert_eq!(vm.mem.u16(unlock + 8).unwrap(), 0b1111);
        assert_eq!(vm.mem.u32(total).unwrap(), 104);
        assert_eq!(fe.call(&mut vm, name_hash("General_TotalQuidditchCards"), &[], 0).unwrap(), 104);
        // clearing card 2 of row 0 keeps its unlock; setting card 3 of row 5 unlocks it too
        let set = |fe: &mut FrontEnd, vm: &mut Vm, a: &[u32]| {
            fe.call(vm, name_hash("General_SetQuidditchCard"), a, 0).unwrap();
        };
        vm.mem.set_u16(unlock, 0x3ff).unwrap();
        set(&mut fe, &mut vm, &[0, 2, 0]);
        assert_eq!((vm.mem.u16(bits).unwrap(), vm.mem.u16(unlock).unwrap()), (0x3fb, 0x3ff));
        vm.mem.set_u16(bits + 10, 0).unwrap();
        set(&mut fe, &mut vm, &[5, 3, 1]);
        assert_eq!((vm.mem.u16(bits + 10).unwrap(), vm.mem.u16(unlock + 10).unwrap()), (8, 8));
    }

    #[test]
    fn the_card_window_takes_a_group_s_texture_and_an_empty_name_gives_it_back() {
        let (mut fe, mut vm) = (FrontEnd::new(), Vm::new());
        let group = vm.mem.alloc_cstr(b"FE_TeamTent_VCPC00");
        let name = vm.mem.alloc_cstr(b"keeper");
        let none = vm.mem.alloc_cstr(b"");
        let hash = name_hash("General_SetQuidditchCardTexture");
        assert_eq!(fe.call(&mut vm, hash, &[group, name], 0).unwrap(), 0);
        assert_eq!(fe.card_texture, Some(("FE_TeamTent_VCPC00".to_string(), "keeper".to_string())));
        fe.call(&mut vm, hash, &[group, none], 0).unwrap();
        assert_eq!(fe.card_texture, None);
    }

    #[test]
    fn game_boy_packets_and_controllers() {
        let (mut fe, mut vm) = (FrontEnd::new(), Vm::new());
        let send = |fe: &mut FrontEnd, vm: &mut Vm, a: &[u32]| {
            assert_eq!(fe.call(vm, name_hash("GBALink_SendQCardPacket"), a, 0).unwrap(), 0);
        };
        // a made-up numbering: row r's bit k is card 10r + k + 1 for the first 10 rows, none after
        let mut t = [[-1i8; 10]; 15];
        for (r, row) in t.iter_mut().enumerate().take(10) {
            for (k, c) in row.iter_mut().enumerate() {
                *c = (10 * r + k + 1) as i8;
            }
        }
        fe.card_numbers = Some(t);
        // owned: row 0 bits 0 and 9 (cards 1 and 10), row 2 bit 3 (card 24), row 12 bit 0 (no card)
        let bits = vm.mem.alloc(30);
        for (r, v) in [(0u32, 0x201u16), (2, 8), (12, 1)] {
            vm.mem.set_u16(bits + 2 * r, v).unwrap();
        }
        fe.datastore.push(Entry { key: KEY_CARD_BITS, kind: 1, count: 15, stride: 2, addr: bits });
        send(&mut fe, &mut vm, &[0, 0x55, 0, 0]);
        // card n is bit (n - 1) % 8 of byte 1 + (n - 1) / 8: 1 -> byte 1 bit 0, 10 -> byte 2 bit 1, 24 -> byte 3 bit 7
        assert_eq!(fe.gba_packet[..5], [0x55, 1, 2, 0x80, 0]);
        // one card's number less 1 (row 2, card 3: 24), then the first four bytes only are cleared
        send(&mut fe, &mut vm, &[0, 0x56, 2, 3]);
        assert_eq!(fe.gba_packet[..5], [0x56, 23, 0, 0, 0]);
        fe.gba_packet[5] = 9;
        send(&mut fe, &mut vm, &[0, 0x57, 0x1ff, 0]);
        assert_eq!(fe.gba_packet[..6], [0x57, 0xff, 0, 0, 0, 9]);
        send(&mut fe, &mut vm, &[0, 0x67, 0, 0]);
        assert_eq!(fe.gba_packet[..2], [0x67, 0]);
        // another kind leaves it
        send(&mut fe, &mut vm, &[0, 0x58, 1, 1]);
        assert_eq!(fe.gba_packet[0], 0x67);
        // players 0 and 1 have controllers while the keyboard and mouse are open
        let present = |fe: &mut FrontEnd, vm: &mut Vm, p: u32| {
            fe.call(vm, name_hash("General_IsPlayersControllerPresent"), &[p], 0).unwrap()
        };
        assert_eq!([0, 1, 2, 0x100].map(|p| present(&mut fe, &mut vm, p)), [1, 1, 0, 1]);
        fe.keyboard_and_mouse = false;
        assert_eq!(present(&mut fe, &mut vm, 0), 0);
    }

    #[test]
    fn entering_the_front_end_places_the_hoops() {
        let (mut fe, mut vm) = (FrontEnd::new(), Vm::new());
        let entry = |fe: &mut FrontEnd, vm: &mut Vm, on: u32| {
            fe.call(vm, name_hash("General_FrontendEntry"), &[on], 0).unwrap();
        };
        // the cut-scene entry turns the scene on but places nothing; entering does, and leaving keeps them
        fe.call(&mut vm, name_hash("General_FrontendCutSceneEntry"), &[0], 0).unwrap();
        assert!(fe.scene && !fe.hoops);
        entry(&mut fe, &mut vm, 1);
        assert!(fe.scene && fe.hoops);
        entry(&mut fe, &mut vm, 0);
        assert!(!fe.scene && fe.hoops);
    }

    #[test]
    fn light_getters_read_back_and_sprites_animate() {
        let (mut fe, mut vm) = (FrontEnd::new(), Vm::new());
        let call = |fe: &mut FrontEnd, vm: &mut Vm, n: &str, a: &[u32]| fe.call(vm, name_hash(n), a, 0).unwrap();
        call(&mut fe, &mut vm, "General_Light_SetDirection_1", &[0, 3f32.to_bits(), 4f32.to_bits()]);
        let out = vm.mem.alloc(12);
        call(&mut fe, &mut vm, "General_Light_GetDirection_1", &[out, out + 4, out + 8]);
        let got: Vec<f32> = (0..3).map(|k| f32::from_bits(vm.mem.u32(out + 4 * k).unwrap())).collect();
        assert!(got.iter().zip([0.0, 0.6, 0.8]).all(|(a, b)| (a - b).abs() < 1e-6), "{got:?}");
        // an animated sprite: label 1 at frame 5 then stop; GotoLabel runs it at once
        let mut o = Object::new(layer::Kind::AnimatedSprite, 0);
        o.program = vec![0x0101_0000, 0x0000_0000, 0x0301_0000, 0x0105_0000, 0];
        o.set_slot(0xc4, 8);
        fe.objects.push(o);
        let h = layer::object_handle(0);
        call(&mut fe, &mut vm, "LayerObj_AnimatedSprite_GotoLabel", &[h, 1]);
        assert_eq!(fe.objects[0].slot(0xc8), 0, "label 1 is not set yet");
        layer::animate(&mut fe.objects[0]);
        assert_eq!((fe.objects[0].slot(0xc8), fe.objects[0].slot(0xd0)), (1, 1));
        fe.objects[0].set_slot(0xd0, 2);
        layer::animate(&mut fe.objects[0]);
        call(&mut fe, &mut vm, "LayerObj_AnimatedSprite_SetFrame", &[h, 0x102]);
        assert_eq!(fe.objects[0].slot(0xc8), 2);
        call(&mut fe, &mut vm, "LayerObj_AnimatedSprite_GotoLabel", &[h, 1]);
        assert_eq!((fe.objects[0].slot(0xc8), fe.objects[0].slot(0xd0)), (5, 4));
        // a 16-bit string's CRC is the CRC of its bytes
        let s = vm.mem.alloc(6);
        vm.mem.write(s, &[b'A', 0, b'b', 0, 0, 0]).unwrap();
        let crc = call(&mut fe, &mut vm, "FECK_CalcCRCFromWCHAR", &[s]);
        assert_eq!(crc, qwc_formats::flow::name_hash_seed(&[b'A', 0, b'b', 0], 0));
        assert!(fe.missing.is_empty());
    }
}
