import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  chiefStaffDelete,
  chiefStaffEnsureSession,
  chiefStaffList,
  chiefStaffSave,
  chiefStaffSetPaused,
  chiefCommitmentsList,
  chiefTasksListAll,
  type ChiefTask,
  type StaffCommitment,
  type StaffProfile,
} from "../lib/api";
import type { StaffHireDraft } from "../components/StaffHireDialog";
import type { StaffRosterItem } from "../components/StaffRoster";
import {
  buildStaffRoster,
  staffProfileFromDraft,
} from "../lib/staffUi";

type StaffEditorState = { profile: StaffProfile | null } | null;

export function useStaffController({
  onSessionReady,
}: {
  onSessionReady?: (sessionId: string) => void;
}) {
  const [profiles, setProfiles] = useState<StaffProfile[]>([]);
  const [tasks, setTasks] = useState<ChiefTask[]>([]);
  const [commitmentsByStaff, setCommitmentsByStaff] = useState<Map<string, StaffCommitment[]>>(
    () => new Map(),
  );
  const [paused, setPaused] = useState(false);
  const [editor, setEditor] = useState<StaffEditorState>(null);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const refreshSeq = useRef(0);
  /** 上一次落到 state 的内容指纹：轮询结果没变就不换引用（否则 App 每 5s 整树重渲染）。 */
  const lastSnapshotRef = useRef("");
  const onSessionReadyRef = useRef(onSessionReady);
  onSessionReadyRef.current = onSessionReady;

  const refresh = useCallback(async () => {
    const seq = ++refreshSeq.current;
    try {
      const [registry, nextTasks] = await Promise.all([
        chiefStaffList(),
        chiefTasksListAll(),
      ]);
      if (seq !== refreshSeq.current) return;
      const commitmentRows = await Promise.all(
        registry.staff.map(async (profile) => {
          try {
            return [profile.staffId, await chiefCommitmentsList(profile.staffId)] as const;
          } catch {
            return [profile.staffId, [] as StaffCommitment[]] as const;
          }
        }),
      );
      if (seq !== refreshSeq.current) return;
      const snapshot = JSON.stringify([registry, nextTasks, commitmentRows]);
      if (snapshot !== lastSnapshotRef.current) {
        lastSnapshotRef.current = snapshot;
        setProfiles(registry.staff);
        setPaused(registry.paused);
        setTasks(nextTasks);
        setCommitmentsByStaff(new Map(commitmentRows));
      }
      setError(null);
    } catch (cause) {
      if (seq !== refreshSeq.current) return;
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  }, []);

  useEffect(() => {
    let disposed = false;
    // 窗口藏在后台 / 最小化时不轮询（与 git 扫描、终端轮询同口径），
    // 可见即补一拍。
    const tick = async () => {
      if (disposed) return;
      if (typeof document !== "undefined" && document.hidden) return;
      await refresh();
    };
    void tick();
    const timer = window.setInterval(() => void tick(), 5000);
    const onVisible = () => void tick();
    document.addEventListener("visibilitychange", onVisible);
    return () => {
      disposed = true;
      window.clearInterval(timer);
      document.removeEventListener("visibilitychange", onVisible);
    };
  }, [refresh]);

  const roster = useMemo(
    () => buildStaffRoster(profiles, tasks, paused, commitmentsByStaff),
    [profiles, tasks, paused, commitmentsByStaff],
  );

  const openHire = useCallback(() => {
    setError(null);
    setEditor({ profile: null });
  }, []);

  const openEdit = useCallback((profile: StaffProfile | null) => {
    if (!profile) return;
    setError(null);
    setEditor({ profile });
  }, []);

  const closeEditor = useCallback(() => {
    if (!saving) setEditor(null);
  }, [saving]);

  const save = useCallback(
    async (draft: StaffHireDraft) => {
      refreshSeq.current += 1;
      const existing = editor?.profile ?? null;
      const profile = staffProfileFromDraft(draft, existing);
      setSaving(true);
      setError(null);
      try {
        const saved = await chiefStaffSave(profile);
        if (!saved) throw new Error("员工配置没有保存");
        await refresh();
        if (!existing) {
          const sessionId = await chiefStaffEnsureSession(saved.staffId);
          if (sessionId) onSessionReadyRef.current?.(sessionId);
        }
        setEditor(null);
      } catch (cause) {
        setError(cause instanceof Error ? cause.message : String(cause));
      } finally {
        setSaving(false);
      }
    },
    [editor?.profile, refresh],
  );

  const select = useCallback(async (member: StaffRosterItem) => {
    refreshSeq.current += 1;
    setError(null);
    try {
      const sessionId = await chiefStaffEnsureSession(member.staffId);
      if (sessionId) onSessionReadyRef.current?.(sessionId);
      await refresh();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  }, [refresh]);

  const togglePause = useCallback(async () => {
    refreshSeq.current += 1;
    setError(null);
    try {
      await chiefStaffSetPaused(!paused);
      await refresh();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  }, [paused, refresh]);

  const remove = useCallback(
    async (staffId: string, opts: { deleteSession?: boolean; clearMemory?: boolean } = {}) => {
      refreshSeq.current += 1;
      setError(null);
      try {
        await chiefStaffDelete(staffId, opts);
        setEditor(null);
        await refresh();
        return true;
      } catch (cause) {
        setError(cause instanceof Error ? cause.message : String(cause));
        return false;
      }
    },
    [refresh],
  );

  return {
    profiles,
    tasks,
    paused,
    roster,
    editor,
    saving,
    error,
    refresh,
    openHire,
    openEdit,
    closeEditor,
    save,
    select,
    togglePause,
    remove,
  };
}
