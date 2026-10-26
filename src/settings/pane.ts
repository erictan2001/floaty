/** One section of the settings window: a rail entry, a heading, and its own render. */
export interface Pane {
  /** Section id and hash route (`#/desktop`). */
  id: string;
  /** Rail label. */
  label: string;
  /** Heading at the top of the pane. */
  title: string;
  /** One line under the heading, explaining what this pane is for. */
  hint?: string;
  /** Fill `host` (a fresh, empty element) with this pane's content. */
  render(host: HTMLElement): void | Promise<void>;
}
