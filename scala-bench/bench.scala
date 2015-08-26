import scala.concurrent.ExecutionContext.Implicits.global
import scala.concurrent._
import scala.concurrent.duration._
import java.util.concurrent.ConcurrentLinkedQueue
import java.util.concurrent.atomic._
import java.util.Stack

import scala.annotation.tailrec

final class MSQueue[A](a: A) {
  private abstract class Q
  private final case class Node(data: A, next: AtomicReference[Q] = new AtomicReference(Emp)) extends Q
  private final case object Emp extends Q
  private val head = new AtomicReference(Node(a))
  private val tail = new AtomicReference(head.get())

  def enq(a: A) {
    val newNode = new Node(a)
    while (true) {
      val curTail = tail.get()
      curTail.next.get()match {
        case n@Node(_,_) => tail.compareAndSet(curTail, n)
        case Emp => {
          if (curTail.next.compareAndSet(Emp, newNode)) {
            tail.compareAndSet(curTail, newNode)
            return
          }
        }
      }
    }
  }

  def deq(): Option[A] = {
    while (true) {
      val cur_head = head.get()
      cur_head.next.get() match {
        case Emp => return None
        case n@Node(data, _) => {
          if (head.compareAndSet(cur_head, n)) {
            return Some(data)
          }
        }
      }
    }
    None
  }
}

abstract class MyBool
final case class MyTrue() extends MyBool
final case class MyFalse() extends MyBool

object Bench {
  def time(block: => Unit): Long = {
    val t0 = System.nanoTime()
    val result = block    // call-by-name
    val t1 = System.nanoTime()
    t1 - t0
  }

  def do_linked(threads: Int, count: Int) {
    val q: ConcurrentLinkedQueue[MyBool] = new ConcurrentLinkedQueue();

    val t = time {
      var s = new Stack[Future[Unit]]
      for (i <- 1 to threads) {
        s.push(Future {
          for (i <- 1 to count+1) {
            //if (i % 100000 == 0) { println(q.size()) }
            q.offer(MyTrue())
          }
        })
      }

      var rn = 0
      while (rn < count) {
        if (q.poll() != null) {
          rn += 1
        }
      }

      while (!s.empty()) {
        Await.ready(s.pop(), Duration.Inf)
      }
    }

    println("Linked: " + t / (count * threads))
  }

  def do_linked_mpmc(threads: Int, count: Int) {
    val q = new ConcurrentLinkedQueue[MyBool];
    val prod = new AtomicInteger();

    val t = time {
      var s = new Stack[Future[Unit]]
      for (i <- 1 to threads) {
        s.push(Future {
          for (i <- 1 to count+1) {
            q.offer(MyTrue())
            //if (i % 100000 == 0) { println(q.size()) }
          }
          if (prod.incrementAndGet() == threads) {
            for (i <- 0 to threads) { q.offer(MyFalse()) }
          }
        })
        s.push(Future {
          var done = false;
          while (!done) {
            q.poll() match {
              case MyFalse() => done = true
              case _ => {}
            }
          }
        })
      }

      while (!s.empty()) {
        Await.ready(s.pop(), Duration.Inf)
      }
    }

    println("Linked mpmc: " + t / (count * threads) + " (" + t / 1000000000 + ")")
  }


  def do_msq(threads: Int, count: Int) {
    val q = new MSQueue[Int](0);

    val t = time {
      var s = new Stack[Future[Unit]]
      for (i <- 1 to threads) {
        s.push(Future { for (i <- 1 to count+1) { q.enq(i) } })
      }

      var rn = 0
      while (rn < count) {
        if (q.deq() != None) {
          rn += 1
        }
      }

      while (!s.empty()) {
        Await.ready(s.pop(), Duration.Inf)
      }
    }

    println("MSQ: " + t / (count * threads) + " (" + t / 1000000000 + ")")
  }

  def do_msq_mpmc(threads: Int, count: Int) {
    val q = new MSQueue[Boolean](true);
    val prod = new AtomicInteger();

    val t = time {
      var s = new Stack[Future[Unit]]
      for (i <- 1 to threads) {
        s.push(Future {
          for (i <- 1 to count+1) { q.enq(true) }
          if (prod.incrementAndGet() == threads) {
            for (i <- 1 to threads) { q.enq(false) }
          }
        })
        s.push(Future {
          var done = false;
          while (!done) {
            q.deq() match {
              case Some(false) => done = true
              case _ => {}
            }
          }
        })
      }

      while (!s.empty()) {
        Await.ready(s.pop(), Duration.Inf)
      }
    }

    println("MSQ mpmc: " + t / (count * threads) + " (" + t / 1000000000 + ")")
  }

  def main(args: Array[String]) {
    do_linked(2, 1000000)
    do_linked(2, 10000000)

    do_msq(2, 1000000)
    do_msq(2, 10000000)

    do_linked_mpmc(2, 1000000)
    do_linked_mpmc(2, 10000000)

    do_msq_mpmc(2, 1000000)
    do_msq_mpmc(2, 10000000)
  }
}
