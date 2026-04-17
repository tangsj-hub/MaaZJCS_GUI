import {
  TaskResultWaitAbortedError,
  TaskResultWaitTimeoutError,
  waitForTaskResult,
} from '@/components/connection/callbackCache';
import { useAppStore } from '@/stores/appStore';
import { normalizeAgentConfigs } from '@/types/interface';
import { loggers } from '@/utils/logger';

import { maaService } from './maaService';

const log = loggers.task;

const taskMonitorControllers = new Map<string, AbortController>();

// 延迟循环任务的增量提交机制
export interface DeferredIteration {
  entry: string;
  pipelineOverride: string;
  selectedTaskId: string;
  displayName: string;
  remaining: number;
  delayMs: number;
}

const deferredIterationMap = new Map<number, DeferredIteration>();

export function registerDeferredIteration(maaTaskId: number, info: DeferredIteration) {
  deferredIterationMap.set(maaTaskId, info);
  log.info(`[task-delay] 注册延迟迭代: maaTaskId=${maaTaskId}, remaining=${info.remaining}, delay=${info.delayMs}ms`);
}

export function clearDeferredIterations() {
  deferredIterationMap.clear();
}

function sleepAbortable(ms: number, signal: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    if (signal.aborted) { reject(new TaskResultWaitAbortedError()); return; }
    const timer = setTimeout(resolve, ms);
    const onAbort = () => { clearTimeout(timer); reject(new TaskResultWaitAbortedError()); };
    signal.addEventListener('abort', onAbort, { once: true });
  });
}

function isAbortError(error: unknown): boolean {
  return error instanceof TaskResultWaitAbortedError;
}

function isTimeoutError(error: unknown): boolean {
  return error instanceof TaskResultWaitTimeoutError;
}

async function stopAgentIfNeeded(instanceId: string) {
  const agentConfigs = normalizeAgentConfigs(useAppStore.getState().projectInterface?.agent);
  if (!agentConfigs || agentConfigs.length === 0) {
    return;
  }

  try {
    await maaService.stopAgent(instanceId);
  } catch (error) {
    log.error(`[task-monitor#${instanceId}] 停止 Agent 失败:`, error);
  }
}

async function finalizeTaskRun(instanceId: string, status: 'Succeeded' | 'Failed') {
  await stopAgentIfNeeded(instanceId);

  const state = useAppStore.getState();
  state.setInstanceTaskStatus(instanceId, status);
  state.updateInstance(instanceId, { isRunning: false });
  state.setInstanceCurrentTaskId(instanceId, null);
  state.clearPendingTasks(instanceId);
  state.clearScheduleExecution(instanceId);
}

function getPendingTaskIds(instanceId: string) {
  return useAppStore.getState().instancePendingTaskIds[instanceId] || [];
}

async function monitorTaskQueue(instanceId: string, controller: AbortController) {
  const initialTaskIds = getPendingTaskIds(instanceId);
  // 运行中可能通过 appendPendingTaskId 动态追加任务，这里只做初始空队列校验。
  if (initialTaskIds.length === 0) {
    log.error(`[task-monitor#${instanceId}] 后端未返回 task_id，终止本次运行`);
    taskMonitorControllers.delete(instanceId);
    await finalizeTaskRun(instanceId, 'Failed');
    return;
  }

  let hasFailed = false;
  let index = 0;

  while (true) {
    if (controller.signal.aborted || taskMonitorControllers.get(instanceId) !== controller) {
      return;
    }

    const taskIds = getPendingTaskIds(instanceId);
    const taskId = taskIds[index];
    if (taskId === undefined) {
      break;
    }

    const state = useAppStore.getState();
    state.setCurrentTaskIndex(instanceId, index);
    state.setInstanceCurrentTaskId(instanceId, taskId);

    const selectedTaskId = state.findSelectedTaskIdByMaaTaskId(instanceId, taskId);
    if (selectedTaskId) {
      state.setTaskRunStatus(instanceId, selectedTaskId, 'running');
    }

    const result = await waitForTaskResult(taskId, { signal: controller.signal });

    if (controller.signal.aborted || taskMonitorControllers.get(instanceId) !== controller) {
      return;
    }

    const latestState = useAppStore.getState();
    const latestSelectedTaskId = latestState.findSelectedTaskIdByMaaTaskId(instanceId, taskId);
    if (latestSelectedTaskId) {
      latestState.setTaskRunStatus(
        instanceId,
        latestSelectedTaskId,
        result === 'succeeded' ? 'succeeded' : 'failed',
      );
    }

    if (result === 'failed') {
      hasFailed = true;
    }

    // 延迟循环：检查是否有剩余迭代需要提交
    const deferred = deferredIterationMap.get(taskId);
    log.info(`[task-delay] 查找延迟迭代: taskId=${taskId}, found=${!!deferred}, mapSize=${deferredIterationMap.size}, mapKeys=[${[...deferredIterationMap.keys()].join(',')}]`);
    if (deferred && deferred.remaining > 0) {
      deferredIterationMap.delete(taskId);

      log.info(`[task-monitor#${instanceId}] 任务 ${taskId} 完成，等待 ${deferred.delayMs}ms 后提交下一次迭代 (剩余 ${deferred.remaining})`);
      useAppStore.getState().addLog(instanceId, { type: 'info', message: `⏳ 循环间隔等待中: ${deferred.delayMs}ms (剩余 ${deferred.remaining} 次)` });
      await sleepAbortable(deferred.delayMs, controller.signal);
      if (controller.signal.aborted || taskMonitorControllers.get(instanceId) !== controller) {
        return;
      }

      // 提交下一次迭代
      useAppStore.getState().addLog(instanceId, { type: 'info', message: `⏳ 等待结束，正在提交下一次迭代...` });
      const newTaskId = await maaService.runTask(instanceId, deferred.entry, deferred.pipelineOverride);
      log.info(`[task-monitor#${instanceId}] 延迟迭代已提交, newTaskId=${newTaskId}`);

      // 注册映射
      const st = useAppStore.getState();
      st.registerMaaTaskMapping(instanceId, newTaskId, deferred.selectedTaskId);
      st.registerTaskIdName(newTaskId, deferred.displayName);
      st.appendPendingTaskId(instanceId, newTaskId);

      // 如果还有更多迭代，为新 taskId 注册下一次
      if (deferred.remaining > 1) {
        deferredIterationMap.set(newTaskId, { ...deferred, remaining: deferred.remaining - 1 });
      }
    }

    index += 1;
  }

  if (taskMonitorControllers.get(instanceId) !== controller) {
    return;
  }

  taskMonitorControllers.delete(instanceId);
  await finalizeTaskRun(instanceId, hasFailed ? 'Failed' : 'Succeeded');
}

export function cancelTaskQueueMonitor(instanceId: string) {
  const controller = taskMonitorControllers.get(instanceId);
  if (!controller) {
    return;
  }

  controller.abort();
  taskMonitorControllers.delete(instanceId);
}

export function startTaskQueueMonitor(instanceId: string) {
  cancelTaskQueueMonitor(instanceId);

  const controller = new AbortController();
  taskMonitorControllers.set(instanceId, controller);

  void monitorTaskQueue(instanceId, controller).catch(async (error) => {
    if (isAbortError(error)) {
      return;
    }

    if (taskMonitorControllers.get(instanceId) === controller) {
      taskMonitorControllers.delete(instanceId);
      if (isTimeoutError(error)) {
        log.error(
          `[task-monitor#${instanceId}] 等待任务结果超时: task_id=${error.taskId}, timeout=${error.timeoutMs}ms`,
        );
      } else {
        log.error(`[task-monitor#${instanceId}] 监视任务队列失败:`, error);
      }
      await finalizeTaskRun(instanceId, 'Failed');
    }
  });
}
