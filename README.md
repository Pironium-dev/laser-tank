
# レーザータンク

![レーザータンク](Tanks.jpg)

Rustで構築された、ハードウェア（ESP32）とPC上のゲームサーバー（Dioxus）が連携する2人プレイ用戦車ゲームです。
組み込みからGUI、通信プロトコルに至るまで、システム全体を **Rust** で統一して開発しています。

---

## ハードウェア仕様 (Hardware)
### 使用部品
* **MCU:** ESP32-WROOM-32E (またはお手持ちのボード)
* **モータードライバ:** [型番]
* **赤外線送信:** 赤外線LED
* **赤外線受信:** [型番]

```mermaid
graph TD
  %% communication モジュール
  subgraph communication
    comm_ServerData["ServerData enum\n- Controller(ControllerState)\n- SetID(u8)"]
    comm_ControllerState["ControllerState struct\n- stick: (i8, i8)\n- shot_id: u8\n- Default"]
    comm_RobotRespond["RobotRespond struct\n- robot_id: u8\n- hit_id: u8"]
    comm_detect["detect_id_change(now: &mut u8, received: u8) -> bool"]
  end

  %% dx の UI と logic
  subgraph dx
    subgraph "dx main (dx/src/main.rs)"
      main_fn["main() -> dioxus::launch(App)"]
      App_comp["App component\n- use_signal / use_future / use_coroutine\n- UI: timer, players, buttons"]
      PlayerArea_comp["PlayerArea component\n- レンダリング専用"]
      SINK["SINK_HANDLE (audio)"]
      logic_coroutine_node["logic_coroutine (use_coroutine)\n- receives ToRobot commands from UI coroutine"]
    end

    subgraph "dx logic (dx/src/logic.rs)"
      logic_init["init() -> returns channels (to_robot_tx, to_server_rx)"]
      enum_ToServer["ToServer enum\n- Connect, Disconnect, Hit, AskShot"]
      enum_ToRobot["ToRobot enum\n- AllowShot, Stop, Start"]
      controller_handler_fn["controller_handler task (async)\n- reads gamepad via gilrs\n- button -> send AskShot to to_server_tx"]
      robot_handler_fn["robot_handler task (async)\n- binds UDP socket\n- recv RobotRespond, manage handlers[2]\n- ticker -> send controller data\n- consumes to_robot_rx"]
      RobotHandler_class["RobotHandler struct\n- fields: robot_id, hit_id, recv_addr, send_addr, socket, controller\n- methods: notify_id(), recv_heartbeat(), send_controller_data(stop_flag)"]
    end
  end

  %% esp32 側（組み込み）
  subgraph esp32
    esp_lib["lib.rs\n- pub mod motor;"]
    motor_mod["motor.rs"]
    motor_Motor["Motor struct (embedded)\n- fields: pin (PWM), phase (GPIO)\n- methods: new(pin, phase), set_velocity(v: f32)\n- depends on esp_hal (GPIO, MCPWM)"]
    esp_hal["esp_hal (HAL for PWM/GPIO)"]
  end

  %% チャネル / ノード定義（特殊文字回避）
  to_robot_tx["to_robot_tx (Sender u8-ToRobot)"]
  to_server_rx["to_server_rx (Receiver u8-ToServer)"]
  to_server_tx["to_server_tx (internal Sender)"]
  gilrs_gamepad["gilrs (gamepad)"]
  UDP_Robot["Robot device (UDP) - sends RobotRespond / receives ServerData"]

  %% 主要ノード間の関係
  App_comp -->|starts coroutine| logic_coroutine_node
  App_comp -->|UI sends control| logic_coroutine_node
  logic_coroutine_node -->|calls init| logic_init
  logic_init --> controller_handler_fn
  logic_init --> robot_handler_fn

  logic_init -->|provides| to_robot_tx
  logic_init -->|provides| to_server_rx

  controller_handler_fn -->|reads| gilrs_gamepad
  controller_handler_fn -->|sends AskShot| to_server_tx
  to_server_tx -->|feeds into| to_server_rx

  robot_handler_fn --> RobotHandler_class
  RobotHandler_class -->|uses| comm_detect
  robot_handler_fn -->|serializes ServerData| comm_ServerData
  robot_handler_fn -->|parses RobotRespond| comm_RobotRespond

  robot_handler_fn -->|send ServerData UDP| UDP_Robot
  UDP_Robot -->|heartbeat / RobotRespond| robot_handler_fn

  UDP_Robot -->|embedded firmware may call| motor_Motor
  motor_Motor --> esp_hal

  logic_coroutine_node -->|sends ToRobot commands| to_robot_tx
  to_robot_tx -->|consumed by| robot_handler_fn

  %% タスクの視覚化（非同期タスク）
  subgraph tasks["async tasks / coroutines"]
    t_controller["Task: controller_handler"]
    t_robot["Task: robot_handler"]
    t_ui_timer["Task: UI timer future (use_future)"]
    t_coroutine["Task: UI coroutine (use_coroutine)"]
  end

  t_controller -->|"to_server (AskShot)"| to_server_tx
  to_server_rx -->|"events (Connect/Disconnect/Hit/AskShot)"| logic_coroutine_node
  t_coroutine -->|sends ToRobot| to_robot_tx
  t_robot -->|manages handlers| RobotHandler_class
  t_ui_timer -->|updates UI signals| App_comp
```
