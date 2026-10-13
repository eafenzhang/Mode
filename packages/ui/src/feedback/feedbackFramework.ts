import {
  DEFAULT_FEEDBACK_TICKET_FRAMEWORK,
  type ModeProvider,
  type FeedbackTicketFramework,
} from "@mode/shared";

export function inferFeedbackFrameworkFromAgentProvider(
  _provider: ModeProvider | null | undefined,
): FeedbackTicketFramework {
  return DEFAULT_FEEDBACK_TICKET_FRAMEWORK;
}
