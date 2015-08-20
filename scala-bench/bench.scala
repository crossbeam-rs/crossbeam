import scala.concurrent.ExecutionContext.Implicits.global
import scala.concurrent._
import scala.concurrent.duration._
import java.util.concurrent.ConcurrentLinkedQueue

object Bench {
  def time(block: => Unit): Long = {
    val t0 = System.nanoTime()
    val result = block    // call-by-name
    val t1 = System.nanoTime()
    t1 - t0
  }

  def doit() {
    val count = 1000000

    val q: ConcurrentLinkedQueue[Int] = new ConcurrentLinkedQueue();

    val t = time {
      val f1 = Future { for (i <- 0 to count) { q.offer(i) } }
      val f2 = Future { for (i <- 0 to count) { q.offer(i) } }
      val f3 = Future { for (i <- 0 to count) { q.offer(i) } }

      var rn = 0
      while (rn < count) {
        if (q.poll() != null) {
          rn += 1
        }
      }

      Await.ready(f1, Duration.Inf)
      Await.ready(f2, Duration.Inf)
      Await.ready(f3, Duration.Inf)
    }

    println(t / (count * 3))
  }

  def main(args: Array[String]) {
    doit()
    doit()
    doit()
    doit()
    doit()
  }
}
