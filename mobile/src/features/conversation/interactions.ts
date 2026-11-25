export interface InteractionQuestion {
  id: string;
  header: string;
  question: string;
  secret: boolean;
  options: { label: string; description: string }[];
}

export interface InteractionField {
  name: string;
  type: 'string' | 'number' | 'integer' | 'boolean' | 'array';
  label: string;
  description: string;
  required: boolean;
  options?: string[];
  optionNames?: string[];
  minLength?: number;
  maxLength?: number;
  minimum?: number;
  maximum?: number;
  format?: string;
}

export interface PendingInteraction {
  id: string;
  source?: 'async-message';
  turnId?: string;
  kind: 'questions' | 'approval' | 'form' | 'desktop';
  title: string;
  message: string;
  detail?: string;
  replySupported: boolean;
  skipSupported?: boolean;
  questions?: InteractionQuestion[];
  fields?: InteractionField[];
  approveLabel?: string;
  desktopMessage?: string;
}

export interface InteractionDraft {
  requestKey: string;
  choices: Record<string, number | undefined>;
  text: Record<string, string>;
  skipped: Record<string, boolean>;
  fields: Record<string, string | number | boolean | string[] | undefined>;
  step: number;
  minimized: boolean;
  busy: boolean;
  error: string;
}

export interface InteractionSubmission {
  threadId: string;
  requestKey: string;
  draft: InteractionDraft;
}

export function interactionRequestKey(threadId: string, entry: PendingInteraction) {
  return JSON.stringify([threadId, entry.turnId || '', entry.id]);
}

export function questionAnswers(entry: PendingInteraction, draft: InteractionDraft) {
  return Object.fromEntries((entry.questions || []).filter(question => !draft.skipped[question.id]).map((question) => {
    const choice = draft.choices[question.id];
    const answer = choice !== undefined && choice >= 0 ? question.options[choice]?.label : draft.text[question.id]?.trim();
    return [question.id, answer ? [answer] : []];
  }));
}

// Keep acknowledgements for this mounted UI session, even while the stream briefly repeats an old request.
export class InteractionReplyState {
  threadId = '';
  entries: PendingInteraction[] = [];
  drafts: Record<string, InteractionDraft> = {};
  settled = new Set<string>();

  sync(threadId: string, entries: PendingInteraction[]) {
    const previous = this.entries;
    if (this.threadId !== threadId) this.drafts = {};
    this.threadId = threadId;
    this.entries = entries;
    for (const entry of entries) {
      const requestKey = interactionRequestKey(threadId, entry);
      const draft = this.drafts[entry.id];
      if (draft?.requestKey === requestKey) {
        if (entry.source === 'async-message') {
          const selected = previous.find(value => value.id === entry.id)?.questions?.[draft.step]?.id;
          draft.step = Math.max(0, entry.questions?.findIndex(question => question.id === selected) ?? 0);
        }
        continue;
      }
      this.drafts[entry.id] = { requestKey, choices: {}, text: {}, skipped: {}, fields: {}, step: 0, minimized: false, busy: false, error: '' };
    }
  }

  get pending() {
    return this.entries.filter((entry) => !this.settled.has(interactionRequestKey(this.threadId, entry)));
  }

  begin(entry: PendingInteraction): InteractionSubmission | undefined {
    const draft = this.drafts[entry.id];
    const requestKey = interactionRequestKey(this.threadId, entry);
    if (!draft || draft.busy || draft.requestKey !== requestKey || !this.pending.some((pending) => interactionRequestKey(this.threadId, pending) === requestKey)) return;
    draft.busy = true;
    draft.error = '';
    return { threadId: this.threadId, requestKey, draft };
  }

  finish(submission: InteractionSubmission) {
    this.settled.add(submission.requestKey);
    submission.draft.busy = false;
  }

  fail(submission: InteractionSubmission, error: string) {
    submission.draft.busy = false;
    submission.draft.error = error;
  }
}
