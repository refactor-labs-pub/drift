import dockerSetupImg from "../../assets/docker_model_runner_setup.png";

type Variant = "not-detected" | "needs-tcp";

export default function DockerSetupHint({ variant }: { variant: Variant }) {
  const summary =
    variant === "needs-tcp"
      ? "Click Apply & Restart in Docker Desktop — show me where"
      : "Enable Docker Model Runner — show me where";
  const body =
    variant === "needs-tcp"
      ? `Docker Desktop → Settings → AI: confirm "Enable host-side TCP support" is on (port 12434), then click Apply & Restart at the bottom. The toggle alone doesn't open the port.`
      : `Docker Desktop → Settings → AI: turn on both "Enable Docker Model Runner" and "Enable host-side TCP support" (port 12434), then click Apply & Restart at the bottom.`;
  return (
    <details className="docker-setup-hint">
      <summary>{summary}</summary>
      <p className="docker-setup-hint-body">{body}</p>
      <img
        src={dockerSetupImg}
        alt="Docker Desktop Settings → AI panel with the Docker Model Runner and host-side TCP toggles and the Apply & Restart button"
        className="docker-setup-hint-img"
      />
    </details>
  );
}
